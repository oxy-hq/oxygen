use std::{
    ops::DerefMut,
    sync::{Arc, RwLock},
};

use chrono::{DateTime, Local, Utc};
use futures::TryFutureExt;
use minijinja::{Environment, Value, context, value::Enumerator};
use tokio::task::spawn_blocking;

use oxy_shared::errors::OxyError;

/// Helper function to add the now() global function to a minijinja Environment.
///
/// The `now()` function is available in all Jinja2 templates (including agent system instructions)
/// and returns the current datetime, optionally formatted via strftime.
///
/// # Usage in Templates
///
/// ```jinja2
/// {{ now() }}                           # Returns current local datetime in RFC3339 format
/// {{ now(utc=true) }}                   # Returns current UTC datetime in RFC3339 format
/// {{ now(fmt='%Y-%m-%d') }}            # Returns current date as "2024-10-21"
/// {{ now(utc=true, fmt='%Y-%m-%d %H:%M:%S') }}  # Returns UTC datetime as "2024-10-21 15:30:45"
/// ```
///
/// # Parameters
///
/// - `utc` (optional, default: false): If true, returns UTC time; otherwise returns local time
/// - `fmt` (optional): A strftime format string for custom datetime formatting
///
/// # Common Format Strings
///
/// - `%Y-%m-%d` - Date (e.g., "2024-10-21")
/// - `%Y-%m-%d %H:%M:%S` - Datetime (e.g., "2024-10-21 15:30:45")
/// - `%A, %B %d, %Y` - Full date (e.g., "Monday, October 21, 2024")
/// - `%I:%M %p` - 12-hour time (e.g., "03:30 PM")
///
fn add_now_function(env: &mut Environment<'static>) {
    env.add_function(
        "now",
        |kwargs: minijinja::value::Kwargs| -> Result<String, minijinja::Error> {
            let utc = kwargs.get::<Option<bool>>("utc")?.unwrap_or(false);
            let fmt = kwargs.get::<Option<String>>("fmt")?;

            let datetime_str = if utc {
                let now: DateTime<Utc> = Utc::now();
                if let Some(format) = fmt {
                    now.format(&format).to_string()
                } else {
                    now.to_rfc3339()
                }
            } else {
                let now: DateTime<Local> = Local::now();
                if let Some(format) = fmt {
                    now.format(&format).to_string()
                } else {
                    now.to_rfc3339()
                }
            };
            Ok(datetime_str)
        },
    );
}

fn add_global_functions(env: &mut Environment<'static>) {
    add_now_function(env);

    env.add_filter(
        "tojson",
        |value: Value| -> Result<String, minijinja::Error> {
            serde_json::to_string(&value).map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("Failed to convert to JSON: {}", e),
                )
            })
        },
    );

    // `sqlquote` writes a value as a `'…'` SQL literal:
    //   {{ controls.store | sqlquote }}  →  'Paris'
    // Do NOT add surrounding quotes in the template — sqlquote provides them.
    //
    // Nothing rendered through this environment is sent to a database from
    // here — control defaults, retrieval text, eval prompts — so the engine
    // that would read the literal is not known, and engines disagree on how a
    // quote or a backslash is written inside one. A value holding either is
    // refused rather than escaped by one engine's rule and broken out of on
    // another's. SQL that runs is rendered by the automation engine, which
    // escapes by the rule of the task's `database`.
    env.add_filter(
        "sqlquote",
        |value: Value| -> Result<String, minijinja::Error> {
            oxy_shared::sql_literal::quote_engine_unknown(&value.to_string()).map_err(|refused| {
                minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, refused.to_string())
            })
        },
    );
}

#[derive(Debug, Clone)]
pub struct Renderer {
    env: Arc<RwLock<Environment<'static>>>,
    global_context: Arc<Value>,
    current_context: Value,
}

impl Renderer {
    pub fn new(global_context: Value) -> Self {
        let env = setup_jinja_environment();

        Renderer {
            env: Arc::new(RwLock::new(env)),
            global_context: Arc::new(global_context),
            current_context: Default::default(),
        }
    }

    pub fn wrap(&self, context: &Value) -> Renderer {
        Renderer {
            env: self.env.clone(),
            global_context: self.global_context.clone(),
            current_context: context! {
              ..Value::from_serialize(&self.current_context),
              ..Value::from_serialize(context),
            },
        }
    }

    pub fn switch_context(&self, global_context: Value, context: Value) -> Renderer {
        let env = setup_jinja_environment();

        Renderer {
            env: Arc::new(RwLock::new(env)),
            global_context: Arc::new(global_context),
            current_context: context,
        }
    }

    pub fn register_template(&self, value: &str) -> Result<(), OxyError> {
        self.env
            .write()?
            .deref_mut()
            .add_template_owned(value.to_string(), value.to_string())
            .map_err(|err| OxyError::RuntimeError(format!("Failed to add template {err}")))?;
        Ok(())
    }

    pub fn render(&self, template: &str) -> Result<String, OxyError> {
        let ctx = self.get_context();
        self.render_sync_internal(template, ctx)
    }

    pub fn render_str(&self, source: &str) -> Result<String, OxyError> {
        let ctx = self.get_context();
        let env = self.env.read()?;
        env.render_str(source, ctx).map_err(|err| {
            OxyError::RuntimeError(format!("Error rendering template string: {err:?}"))
        })
    }

    pub fn render_once(&self, template: &str, context: Value) -> Result<String, OxyError> {
        self.register_template(template)?;
        self.render_sync_internal(template, context)
    }

    pub async fn render_async(&self, template: &str) -> Result<String, OxyError> {
        self.render_async_internal(template, self.get_context())
            .await
    }

    pub async fn render_once_async(
        &mut self,
        template: &str,
        context: Value,
    ) -> Result<String, OxyError> {
        self.register_template(template)?;
        self.render_async_internal(template, context).await
    }

    pub fn eval_expression(&self, template: &str) -> Result<Value, OxyError> {
        let env = self.env.read()?;
        let variable_regex = regex::Regex::new(r"^\{\{(.*)\}\}$")
            .map_err(|err| OxyError::RuntimeError(format!("Invalid regex: {err}")))?;
        let variable = variable_regex.replace(template.trim(), "$1").to_string();
        let expression = env.compile_expression(&variable).map_err(|err| {
            OxyError::RuntimeError(format!("Failed to compile expression {template} :{err}"))
        })?;
        let context = self.get_context();
        let value = expression.eval(&context).map_err(|err| {
            OxyError::RuntimeError(format!(
                "Error evaluating expression: {} with context: {:?}",
                err, context
            ))
        })?;
        tracing::debug!(
            "Evaluated expression: {} -> {:?} with context: {:?}",
            template,
            value,
            &context
        );
        Ok(value)
    }

    pub fn eval_enumerate(&self, template: &str) -> Result<Vec<Value>, OxyError> {
        let rendered = self.eval_expression(template)?;
        let rendered_value = match rendered.as_object() {
            Some(obj) => obj,
            None => {
                return Err(OxyError::RuntimeError(format!(
                    "Values {template} did not resolve to an object",
                )));
            }
        };

        match rendered_value.enumerate() {
            Enumerator::Seq(length) => {
                let mut values = Vec::new();
                for idx in 0..length {
                    let value = rendered_value
                        .get_value(&Value::from(idx))
                        .unwrap_or_default();
                    values.push(value);
                }
                Ok(values)
            }
            _ => Err(OxyError::RuntimeError(format!(
                "Values {} did not resolve to an array. \nContext: {}",
                template,
                self.get_context()
            ))),
        }
    }

    async fn render_async_internal(
        &self,
        template: &str,
        context: Value,
    ) -> Result<String, OxyError> {
        let env = self.env.clone();
        let template = template.to_string();
        spawn_blocking(move || {
            let env = env.read()?;
            let tmpl = match env.get_template(&template) {
                Ok(tmpl) => tmpl,
                Err(err) => {
                    return Err(OxyError::ConfigurationError(format!(
                        "Template \"{template}\" not found: {err}"
                    )));
                }
            };
            tmpl.render(context)
                .map_err(|err| OxyError::RuntimeError(format!("Error rendering template: {err:?}")))
        })
        .map_err(|err| OxyError::RuntimeError(format!("Error rendering template: {err:?}")))
        .await?
    }

    fn render_sync_internal(&self, template: &str, context: Value) -> Result<String, OxyError> {
        let env = self.env.write()?;
        let tmpl = env.get_template(template).map_err(|err| {
            OxyError::ConfigurationError(format!("Template \"{template}\" not found: {err}"))
        })?;
        tmpl.render(context)
            .map_err(|err| OxyError::RuntimeError(format!("Error rendering template: {err:?}")))
    }

    pub fn get_context(&self) -> Value {
        context! {
          ..Value::from_serialize(self.global_context.as_ref()),
          ..Value::from_serialize(&self.current_context),
        }
    }
}

pub fn setup_jinja_environment() -> Environment<'static> {
    let mut env = Environment::new();
    add_global_functions(&mut env);
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::context;

    #[test]
    fn test_now_function_default() {
        let renderer = Renderer::new(context! {});
        let template = "{{ now() }}";
        renderer.register_template(template).unwrap();
        let result = renderer.render(template).unwrap();
        // Should return a non-empty datetime string
        assert!(!result.is_empty());
    }

    #[test]
    fn test_now_function_with_utc() {
        let renderer = Renderer::new(context! {});
        let template = "{{ now(utc=true) }}";
        renderer.register_template(template).unwrap();
        let result = renderer.render(template).unwrap();
        // Should return a non-empty UTC datetime string
        assert!(!result.is_empty());
    }

    #[test]
    fn test_now_function_with_format() {
        let renderer = Renderer::new(context! {});
        let template = "{{ now(fmt='%Y-%m-%d') }}";
        renderer.register_template(template).unwrap();
        let result = renderer.render(template).unwrap();
        // Should return a date in YYYY-MM-DD format
        assert_eq!(result.len(), 10, "Expected YYYY-MM-DD format"); // YYYY-MM-DD is 10 characters
        assert!(result.contains('-'));
    }

    #[test]
    fn test_now_function_with_utc_and_format() {
        let renderer = Renderer::new(context! {});
        let template = "{{ now(utc=true, fmt='%Y-%m-%d %H:%M:%S') }}";
        renderer.register_template(template).unwrap();
        let result = renderer.render(template).unwrap();
        // Should return a datetime in YYYY-MM-DD HH:MM:SS format
        assert_eq!(result.len(), 19, "Expected YYYY-MM-DD HH:MM:SS format"); // "YYYY-MM-DD HH:MM:SS" is 19 characters
    }

    fn sqlquote(value: &str) -> Result<String, OxyError> {
        Renderer::new(context! { v => value }).render_str("x = {{ v | sqlquote }} AND tail")
    }

    #[test]
    fn sqlquote_writes_a_plain_value_unchanged() {
        assert_eq!(sqlquote("Paris").unwrap(), "x = 'Paris' AND tail");
        assert_eq!(sqlquote("2024-01-01").unwrap(), "x = '2024-01-01' AND tail");
        assert_eq!(sqlquote("").unwrap(), "x = '' AND tail");
    }

    /// No engine is known to this renderer, and a quote-doubled literal ends
    /// early on an engine that reads a backslash (`C:\`, `x\' OR 1=1 -- `)
    /// or one that does not read `''` (`it's` on BigQuery). So none is written.
    #[test]
    fn sqlquote_refuses_a_value_engines_read_differently() {
        for value in ["a\\b", "it's", "x\\' OR 1=1 -- ", "C:\\"] {
            let refused = sqlquote(value).unwrap_err().to_string();
            assert!(refused.contains("is not known"), "{value:?}: {refused}");
        }
    }
}
