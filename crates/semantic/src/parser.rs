use crate::SemanticLayerError;
use crate::models::*;
use crate::validation::{ValidationResult, validate_semantic_layer, validate_variable_syntax};
use crate::variables::VariableEncoder;
use serde_yaml;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Configuration for the semantic model parser
#[derive(Debug, Clone)]
pub struct ParserConfig {
    /// Base directory containing the semantic model files
    pub base_path: PathBuf,
    /// Whether to validate files during parsing
    pub validate: bool,
    /// Whether to follow symbolic links
    pub follow_symlinks: bool,
}

impl ParserConfig {
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            validate: true,
            follow_symlinks: false,
        }
    }

    pub fn with_validation(mut self, validate: bool) -> Self {
        self.validate = validate;
        self
    }

    pub fn with_symlinks(mut self, follow_symlinks: bool) -> Self {
        self.follow_symlinks = follow_symlinks;
        self
    }
}

/// Result of parsing semantic model files
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub semantic_layer: SemanticLayer,
    /// Validation result if validation was enabled
    pub validation: Option<ValidationResult>,
    pub parsed_files: Vec<PathBuf>,
    /// All variables found across the semantic model
    pub variables_found: HashSet<String>,
}

pub struct SemanticLayerParser {
    config: ParserConfig,
}

impl SemanticLayerParser {
    /// Creates a new parser with the given configuration
    pub fn new(config: ParserConfig) -> Self {
        Self { config }
    }

    /// Parses the semantic model from the configured directory structure.
    /// Recursively scans the base path for `.view.yml`/`.view.yaml` and
    /// `.topic.yml`/`.topic.yaml` files, so they can live anywhere under
    /// the base path (flat, nested, etc.).
    pub fn parse(&self) -> Result<ParseResult, SemanticLayerError> {
        let mut views = Vec::new();
        let mut topics = Vec::new();
        let mut parsed_files = Vec::new();

        if !self.config.base_path.exists() {
            return Err(SemanticLayerError::IOError(format!(
                "Base path not found: {}",
                self.config.base_path.display()
            )));
        }

        // Track visited canonical paths to avoid symlink cycles
        let mut visited = HashSet::new();

        // Recursively discover all view and topic files under the base path
        let mut dirs_to_scan = vec![self.config.base_path.clone()];
        while let Some(dir) = dirs_to_scan.pop() {
            // Resolve canonical path to detect cycles.
            // If canonicalization fails (e.g. permission error), fall back to
            // the unresolved path.  Cycle detection is best-effort in that case.
            let canonical = dir.canonicalize().unwrap_or_else(|e| {
                warn!(
                    path = %dir.display(),
                    error = %e,
                    "canonicalize failed; symlink cycle detection may be incomplete"
                );
                dir.clone()
            });
            if !visited.insert(canonical) {
                continue;
            }

            let entries = fs::read_dir(&dir).map_err(|e| {
                SemanticLayerError::IOError(format!(
                    "Failed to read directory {}: {}",
                    dir.display(),
                    e
                ))
            })?;

            for entry in entries {
                let entry = entry.map_err(|e| {
                    SemanticLayerError::IOError(format!("Failed to read directory entry: {}", e))
                })?;
                let path = entry.path();

                if path.is_dir() {
                    // Skip symlinked directories unless follow_symlinks is enabled
                    if path.is_symlink() && !self.config.follow_symlinks {
                        continue;
                    }
                    // Skip hidden directories and common non-project directories
                    if let Some(name) = path.file_name().and_then(|n| n.to_str())
                        && (name.starts_with('.')
                            || name == "target"
                            || name == "node_modules"
                            || name == "dist"
                            || name == "build")
                    {
                        continue;
                    }
                    dirs_to_scan.push(path);
                } else if path.is_file() {
                    if self.is_view_file(&path) {
                        let view = self.parse_view_file(&path)?;
                        views.push(view);
                        parsed_files.push(path);
                    } else if self.is_topic_file(&path) {
                        let topic = self.parse_topic_file(&path)?;
                        topics.push(topic);
                        parsed_files.push(path);
                    }
                }
            }
        }

        self.assemble(views, topics, parsed_files)
    }

    /// Parses a model whose files are already in memory: `(path, YAML text)`
    /// pairs, the path naming the file in errors and in `parsed_files`.
    ///
    /// Same validation and variable collection as [`Self::parse`], which reads
    /// the same shape from a directory.
    pub fn parse_sources(
        &self,
        views: &[(PathBuf, String)],
        topics: &[(PathBuf, String)],
    ) -> Result<ParseResult, SemanticLayerError> {
        let mut parsed_views = Vec::with_capacity(views.len());
        let mut parsed_topics = Vec::with_capacity(topics.len());
        let mut parsed_files = Vec::with_capacity(views.len() + topics.len());
        for (path, content) in views {
            parsed_views.push(self.parse_view_str(content, path)?);
            parsed_files.push(path.clone());
        }
        for (path, content) in topics {
            parsed_topics.push(self.parse_topic_str(content, path)?);
            parsed_files.push(path.clone());
        }
        self.assemble(parsed_views, parsed_topics, parsed_files)
    }

    /// Validate parsed views and topics and collect the variables they use.
    fn assemble(
        &self,
        views: Vec<View>,
        topics: Vec<Topic>,
        parsed_files: Vec<PathBuf>,
    ) -> Result<ParseResult, SemanticLayerError> {
        if views.is_empty() && self.config.validate {
            return Err(SemanticLayerError::IOError(format!(
                "At least one .view.yml file is required; .topic.yml files alone are not \
                 sufficient. No view files (*.view.yml or *.view.yaml) found under: {}",
                self.config.base_path.display()
            )));
        }

        let semantic_layer = SemanticLayer {
            views,
            topics: if topics.is_empty() {
                None
            } else {
                Some(topics)
            },
            metadata: None,
        };

        // Validate if enabled
        let validation = if self.config.validate {
            Some(validate_semantic_layer(&semantic_layer)?)
        } else {
            None
        };

        // Collect all variables used across the semantic model
        let mut variables_found = HashSet::new();
        let encoder = VariableEncoder::new();

        for view in &semantic_layer.views {
            for dimension in &view.dimensions {
                if dimension.has_variables() {
                    let vars = encoder.extract_variables(&dimension.expr);
                    variables_found.extend(vars);
                }
            }
            // Collect variables from measures
            if let Some(measures) = &view.measures {
                for measure in measures {
                    if measure.has_variables() {
                        if let Some(expr) = &measure.expr {
                            let vars = encoder.extract_variables(expr);
                            variables_found.extend(vars);
                        }
                        // Also check filters
                        if let Some(filters) = &measure.filters {
                            for filter in filters {
                                if filter.has_variables() {
                                    let vars = encoder.extract_variables(&filter.expr);
                                    variables_found.extend(vars);
                                }
                            }
                        }
                    }
                }
            }
            // Collect variables from table references
            if let Some(table) = &view.table
                && encoder.has_variables(table)
            {
                let vars = encoder.extract_variables(table);
                variables_found.extend(vars);
            }
            if let Some(sql) = &view.sql
                && encoder.has_variables(sql)
            {
                let vars = encoder.extract_variables(sql);
                variables_found.extend(vars);
            }
        }

        // Topics don't have their own dimensions/measures, they reference views
        // So no need to check topics for variables

        Ok(ParseResult {
            semantic_layer,
            validation,
            parsed_files,
            variables_found,
        })
    }

    pub fn parse_view_file(&self, path: &Path) -> Result<View, SemanticLayerError> {
        // Read raw YAML content as a string
        let content = fs::read_to_string(path).map_err(|e| {
            SemanticLayerError::IOError(format!("Failed to read file {}: {}", path.display(), e))
        })?;
        self.parse_view_str(&content, path)
    }

    /// Parses one view from YAML text already in memory. `path` is only used to
    /// name the file in error messages; nothing is read from it.
    pub fn parse_view_str(&self, content: &str, path: &Path) -> Result<View, SemanticLayerError> {
        reject_legacy_globals(content, path)?;

        let mut yaml_value: serde_yaml::Value = serde_yaml::from_str(content).map_err(|e| {
            let location_info = if let Some(location) = e.location() {
                format!(" at line {}, column {}", location.line(), location.column())
            } else {
                String::new()
            };
            SemanticLayerError::ParsingError(format!(
                "Failed to parse YAML in {}{}: {}",
                path.display(),
                location_info,
                e
            ))
        })?;

        // Store original expressions before any variable processing
        yaml_value = self.preprocess_variables(&yaml_value, path)?;

        // Now deserialize into View struct
        let view: View = serde_yaml::from_value(yaml_value).map_err(|e| {
            let location_info = if let Some(location) = e.location() {
                format!(" at line {}, column {}", location.line(), location.column())
            } else {
                String::new()
            };
            SemanticLayerError::ParsingError(format!(
                "Failed to deserialize view from {}{}: {}",
                path.display(),
                location_info,
                e
            ))
        })?;

        Ok(view)
    }

    pub fn parse_topic_file(&self, path: &Path) -> Result<Topic, SemanticLayerError> {
        let content = fs::read_to_string(path).map_err(|e| {
            SemanticLayerError::IOError(format!("Failed to read file {}: {}", path.display(), e))
        })?;
        self.parse_topic_str(&content, path)
    }

    /// Parses one topic from YAML text already in memory. See
    /// [`Self::parse_view_str`].
    pub fn parse_topic_str(&self, content: &str, path: &Path) -> Result<Topic, SemanticLayerError> {
        reject_legacy_globals(content, path)?;

        // First parse as generic YAML value
        let yaml_value: serde_yaml::Value = serde_yaml::from_str(content).map_err(|e| {
            let location_info = if let Some(location) = e.location() {
                format!(" at line {}, column {}", location.line(), location.column())
            } else {
                String::new()
            };
            SemanticLayerError::ParsingError(format!(
                "Failed to parse YAML in {}{}: {}",
                path.display(),
                location_info,
                e
            ))
        })?;

        // Topics don't currently have direct variable support in their structure,
        // but validate any variable syntax if present for future compatibility
        self.validate_topic_variables(&yaml_value, path)?;

        // Now deserialize into Topic struct
        let topic: Topic = serde_yaml::from_value(yaml_value).map_err(|e| {
            let location_info = if let Some(location) = e.location() {
                format!(" at line {}, column {}", location.line(), location.column())
            } else {
                String::new()
            };
            SemanticLayerError::ParsingError(format!(
                "Failed to deserialize topic from {}{}: {}",
                path.display(),
                location_info,
                e
            ))
        })?;

        Ok(topic)
    }

    /// Checks if a file is a view file based on its extension
    fn is_view_file(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext == "yaml" || ext == "yml")
            .unwrap_or(false)
            && path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.ends_with(".view"))
                .unwrap_or(false)
    }

    /// Checks if a file is a topic file based on its extension
    fn is_topic_file(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext == "yaml" || ext == "yml")
            .unwrap_or(false)
            && path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.ends_with(".topic"))
                .unwrap_or(false)
    }

    /// Preprocesses variables in YAML value to store original expressions and validate syntax
    fn preprocess_variables(
        &self,
        yaml_value: &serde_yaml::Value,
        file_path: &Path,
    ) -> Result<serde_yaml::Value, SemanticLayerError> {
        let mut processed_value = yaml_value.clone();
        let encoder = VariableEncoder::new();

        if let Some(dimensions) = processed_value.get_mut("dimensions")
            && let Some(dimensions_array) = dimensions.as_sequence_mut()
        {
            for dimension in dimensions_array {
                if let Some(dimension_map) = dimension.as_mapping_mut()
                    && let Some(expr_value) = dimension_map.get("expr")
                    && let Some(expr_str) = expr_value.as_str()
                {
                    let validation = validate_variable_syntax(
                        expr_str,
                        &format!("Dimension in {}", file_path.display()),
                    );
                    if !validation.is_valid {
                        return Err(SemanticLayerError::ValidationError(
                            validation.errors.join("; "),
                        ));
                    }

                    if encoder.has_variables(expr_str) {
                        dimension_map.insert(
                            serde_yaml::Value::String("original_expr".to_string()),
                            serde_yaml::Value::String(expr_str.to_string()),
                        );
                    }
                }
            }
        }

        if let Some(measures) = processed_value.get_mut("measures")
            && let Some(measures_array) = measures.as_sequence_mut()
        {
            for measure in measures_array {
                if let Some(measure_map) = measure.as_mapping_mut() {
                    // Process measure expression
                    if let Some(expr_value) = measure_map.get("expr")
                        && let Some(expr_str) = expr_value.as_str()
                    {
                        let validation = validate_variable_syntax(
                            expr_str,
                            &format!("Measure in {}", file_path.display()),
                        );
                        if !validation.is_valid {
                            return Err(SemanticLayerError::ValidationError(
                                validation.errors.join("; "),
                            ));
                        }

                        if encoder.has_variables(expr_str) {
                            measure_map.insert(
                                serde_yaml::Value::String("original_expr".to_string()),
                                serde_yaml::Value::String(expr_str.to_string()),
                            );
                        }
                    }

                    if let Some(filters) = measure_map.get_mut("filters")
                        && let Some(filters_array) = filters.as_sequence_mut()
                    {
                        for filter in filters_array {
                            if let Some(filter_map) = filter.as_mapping_mut()
                                && let Some(expr_value) = filter_map.get("expr")
                                && let Some(expr_str) = expr_value.as_str()
                            {
                                let validation = validate_variable_syntax(
                                    expr_str,
                                    &format!("Measure filter in {}", file_path.display()),
                                );
                                if !validation.is_valid {
                                    return Err(SemanticLayerError::ValidationError(
                                        validation.errors.join("; "),
                                    ));
                                }

                                if encoder.has_variables(expr_str) {
                                    filter_map.insert(
                                        serde_yaml::Value::String("original_expr".to_string()),
                                        serde_yaml::Value::String(expr_str.to_string()),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(table_value) = processed_value.get("table")
            && let Some(table_str) = table_value.as_str()
        {
            let validation = validate_variable_syntax(
                table_str,
                &format!("Table reference in {}", file_path.display()),
            );
            if !validation.is_valid {
                return Err(SemanticLayerError::ValidationError(
                    validation.errors.join("; "),
                ));
            }
        }

        if let Some(sql_value) = processed_value.get("sql")
            && let Some(sql_str) = sql_value.as_str()
        {
            let validation =
                validate_variable_syntax(sql_str, &format!("SQL query in {}", file_path.display()));
            if !validation.is_valid {
                return Err(SemanticLayerError::ValidationError(
                    validation.errors.join("; "),
                ));
            }
        }

        Ok(processed_value)
    }

    /// Validates variable syntax in topic files (for future compatibility)
    fn validate_topic_variables(
        &self,
        yaml_value: &serde_yaml::Value,
        file_path: &Path,
    ) -> Result<(), SemanticLayerError> {
        // Topics don't currently support direct variables, but we validate
        // any variable-like syntax for future compatibility and clear error messages

        // Check string values recursively for variable syntax
        fn check_value(value: &serde_yaml::Value, context: &str) -> Result<(), String> {
            match value {
                serde_yaml::Value::String(s) => {
                    let validation = validate_variable_syntax(s, context);
                    if !validation.is_valid {
                        return Err(validation.errors.join("; "));
                    }
                }
                serde_yaml::Value::Mapping(map) => {
                    for (key, val) in map {
                        if let Some(key_str) = key.as_str() {
                            let new_context = format!("{}.{}", context, key_str);
                            check_value(val, &new_context)?;
                        }
                    }
                }
                serde_yaml::Value::Sequence(seq) => {
                    for (i, val) in seq.iter().enumerate() {
                        let new_context = format!("{}[{}]", context, i);
                        check_value(val, &new_context)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }

        check_value(yaml_value, &format!("Topic in {}", file_path.display()))
            .map_err(SemanticLayerError::ValidationError)?;

        Ok(())
    }
}

/// Reject view/topic files that still reference the removed globals/inheritance features.
///
/// Without this guard, leftover `inherits_from:` keys are silently dropped (View/Topic
/// don't declare the field) and `{{ globals.* }}` Jinja expressions survive YAML parsing
/// as literal strings, producing semantically wrong views with no visible error.
fn reject_legacy_globals(content: &str, path: &Path) -> Result<(), SemanticLayerError> {
    let has_inherits_from = content
        .lines()
        .any(|line| line.trim_start().starts_with("inherits_from:"));
    let has_globals_template = content.contains("{{globals.") || content.contains("{{ globals.");

    if has_inherits_from || has_globals_template {
        return Err(SemanticLayerError::ParsingError(format!(
            "{} uses removed semantic-model features (`inherits_from:` and/or \
             `{{{{ globals.* }}}}`). The semantics.yml registry has been removed; \
             inline the referenced values directly into the file.",
            path.display()
        )));
    }
    Ok(())
}

pub fn parse_semantic_layer_from_dir<P: AsRef<Path>>(
    path: P,
) -> Result<ParseResult, SemanticLayerError> {
    let parser_config = ParserConfig::new(path);
    let parser = SemanticLayerParser::new(parser_config);
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Creates a test fixture with a minimal semantic model structure.
    fn create_test_fixture() -> TempDir {
        TempDir::new().unwrap()
    }

    fn minimal_view_yaml(name: &str) -> String {
        format!(
            r#"name: {name}
description: Test view for {name}
table: test_table
entities: []
dimensions:
  - name: id
    expr: id
    type: number
"#,
            name = name
        )
    }

    mod in_memory_tests {
        use super::*;

        fn parser() -> SemanticLayerParser {
            SemanticLayerParser::new(ParserConfig::new("semantics"))
        }

        /// A model whose files are text already in memory parses to the same
        /// layer the directory walk builds from the same documents, and a
        /// document that does not parse names the file it came from.
        #[test]
        fn parse_sources_matches_parsing_the_same_files_from_a_directory() {
            let dir = create_test_fixture();
            std::fs::write(
                dir.path().join("orders.view.yml"),
                minimal_view_yaml("orders"),
            )
            .unwrap();
            std::fs::write(
                dir.path().join("items.view.yml"),
                minimal_view_yaml("items"),
            )
            .unwrap();
            let from_dir = parse_semantic_layer_from_dir(dir.path()).unwrap();

            // The directory walk's order is the file system's; sort both.
            let mut from_dir_names: Vec<String> = from_dir
                .semantic_layer
                .views
                .iter()
                .map(|v| v.name.clone())
                .collect();
            from_dir_names.sort();

            let views = vec![
                (
                    PathBuf::from("views/orders.view.yml"),
                    minimal_view_yaml("orders"),
                ),
                (
                    PathBuf::from("views/items.view.yml"),
                    minimal_view_yaml("items"),
                ),
            ];
            let from_text = parser().parse_sources(&views, &[]).unwrap();
            let mut from_text_names: Vec<String> = from_text
                .semantic_layer
                .views
                .iter()
                .map(|v| v.name.clone())
                .collect();
            from_text_names.sort();

            assert_eq!(from_text_names, from_dir_names);
            assert_eq!(from_text.parsed_files.len(), 2);
            assert_eq!(from_text.variables_found, from_dir.variables_found);
            assert!(from_text.validation.is_some(), "validation still runs");
        }

        #[test]
        fn a_source_that_does_not_parse_names_its_path() {
            let views = vec![(
                PathBuf::from("views/bad.view.yml"),
                "name: [unclosed".to_string(),
            )];
            let err = parser().parse_sources(&views, &[]).unwrap_err().to_string();
            assert!(err.contains("views/bad.view.yml"), "{err}");
        }

        #[test]
        fn a_model_with_no_views_is_refused_when_validating() {
            let err = parser().parse_sources(&[], &[]).unwrap_err().to_string();
            assert!(err.contains("At least one .view.yml"), "{err}");
        }

        #[test]
        fn parse_view_str_reads_no_file() {
            let view = parser()
                .parse_view_str(
                    &minimal_view_yaml("orders"),
                    Path::new("/does/not/exist.view.yml"),
                )
                .unwrap();
            assert_eq!(view.name, "orders");
        }
    }

    mod parser_config_tests {
        use super::*;

        #[test]
        fn test_new_creates_config_with_defaults() {
            let config = ParserConfig::new("/some/path");
            assert_eq!(config.base_path, PathBuf::from("/some/path"));
            assert!(config.validate);
            assert!(!config.follow_symlinks);
        }

        #[test]
        fn test_with_validation_sets_validation() {
            let config = ParserConfig::new("/path").with_validation(false);
            assert!(!config.validate);
        }

        #[test]
        fn test_with_symlinks_sets_follow_symlinks() {
            let config = ParserConfig::new("/path").with_symlinks(true);
            assert!(config.follow_symlinks);
        }

        #[test]
        fn test_builder_chaining() {
            let config = ParserConfig::new("/path")
                .with_validation(false)
                .with_symlinks(true);
            assert!(!config.validate);
            assert!(config.follow_symlinks);
        }
    }

    mod is_view_file_tests {
        use super::*;

        fn check_is_view_file(filename: &str) -> bool {
            let temp_dir = create_test_fixture();
            let config = ParserConfig::new(temp_dir.path());
            let parser = SemanticLayerParser::new(config);
            parser.is_view_file(&PathBuf::from(filename))
        }

        #[test]
        fn test_valid_view_yaml_extension() {
            assert!(check_is_view_file("orders.view.yaml"));
        }

        #[test]
        fn test_valid_view_yml_extension() {
            assert!(check_is_view_file("orders.view.yml"));
        }

        #[test]
        fn test_invalid_no_view_suffix() {
            assert!(!check_is_view_file("orders.yaml"));
        }

        #[test]
        fn test_invalid_wrong_extension() {
            assert!(!check_is_view_file("orders.view.json"));
        }

        #[test]
        fn test_invalid_topic_file() {
            assert!(!check_is_view_file("orders.topic.yaml"));
        }
    }

    mod is_topic_file_tests {
        use super::*;

        fn check_is_topic_file(filename: &str) -> bool {
            let temp_dir = create_test_fixture();
            let config = ParserConfig::new(temp_dir.path());
            let parser = SemanticLayerParser::new(config);
            parser.is_topic_file(&PathBuf::from(filename))
        }

        #[test]
        fn test_valid_topic_yaml_extension() {
            assert!(check_is_topic_file("sales.topic.yaml"));
        }

        #[test]
        fn test_valid_topic_yml_extension() {
            assert!(check_is_topic_file("sales.topic.yml"));
        }

        #[test]
        fn test_invalid_no_topic_suffix() {
            assert!(!check_is_topic_file("sales.yaml"));
        }

        #[test]
        fn test_invalid_view_file() {
            assert!(!check_is_topic_file("sales.view.yaml"));
        }
    }

    mod parse_tests {
        use super::*;

        #[test]
        fn test_parse_missing_base_dir_fails() {
            let config = ParserConfig::new("/nonexistent/semantics");
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse();
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(matches!(err, SemanticLayerError::IOError(_)));
        }

        #[test]
        fn test_parse_no_view_files_with_validation_fails() {
            let temp_dir = create_test_fixture();
            let config = ParserConfig::new(temp_dir.path()).with_validation(true);
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse();
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(matches!(err, SemanticLayerError::IOError(_)));
        }

        #[test]
        fn test_parse_no_view_files_without_validation_returns_empty() {
            let temp_dir = create_test_fixture();
            let config = ParserConfig::new(temp_dir.path()).with_validation(false);
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse();
            assert!(result.is_ok());
            let parse_result = result.unwrap();
            assert!(parse_result.semantic_layer.views.is_empty());
        }

        #[test]
        fn test_parse_single_view_file_flat() {
            let temp_dir = create_test_fixture();
            std::fs::write(
                temp_dir.path().join("orders.view.yaml"),
                minimal_view_yaml("orders"),
            )
            .unwrap();

            let config = ParserConfig::new(temp_dir.path()).with_validation(false);
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse();
            assert!(result.is_ok());
            let parse_result = result.unwrap();
            assert_eq!(parse_result.semantic_layer.views.len(), 1);
            assert_eq!(parse_result.semantic_layer.views[0].name, "orders");
            assert_eq!(parse_result.parsed_files.len(), 1);
        }

        #[test]
        fn test_parse_single_view_file_in_subdirectory() {
            let temp_dir = create_test_fixture();
            let views_dir = temp_dir.path().join("views");
            std::fs::create_dir(&views_dir).unwrap();
            std::fs::write(
                views_dir.join("orders.view.yaml"),
                minimal_view_yaml("orders"),
            )
            .unwrap();

            let config = ParserConfig::new(temp_dir.path()).with_validation(false);
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse();
            assert!(result.is_ok());
            let parse_result = result.unwrap();
            assert_eq!(parse_result.semantic_layer.views.len(), 1);
            assert_eq!(parse_result.semantic_layer.views[0].name, "orders");
            assert_eq!(parse_result.parsed_files.len(), 1);
        }

        #[test]
        fn test_parse_deeply_nested_view_file() {
            let temp_dir = create_test_fixture();
            let nested_dir = temp_dir.path().join("semantics/domain/subdomain");
            std::fs::create_dir_all(&nested_dir).unwrap();
            std::fs::write(
                nested_dir.join("orders.view.yaml"),
                minimal_view_yaml("orders"),
            )
            .unwrap();

            let config = ParserConfig::new(temp_dir.path()).with_validation(false);
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse();
            assert!(result.is_ok());
            let parse_result = result.unwrap();
            assert_eq!(parse_result.semantic_layer.views.len(), 1);
            assert_eq!(parse_result.semantic_layer.views[0].name, "orders");
        }

        #[test]
        fn test_parse_multiple_view_files() {
            let temp_dir = create_test_fixture();

            std::fs::write(
                temp_dir.path().join("orders.view.yaml"),
                minimal_view_yaml("orders"),
            )
            .unwrap();
            std::fs::write(
                temp_dir.path().join("users.view.yaml"),
                minimal_view_yaml("users"),
            )
            .unwrap();

            let config = ParserConfig::new(temp_dir.path()).with_validation(false);
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse();
            assert!(result.is_ok());
            let parse_result = result.unwrap();
            assert_eq!(parse_result.semantic_layer.views.len(), 2);
            assert_eq!(parse_result.parsed_files.len(), 2);
        }

        #[test]
        fn test_parse_ignores_non_view_files() {
            let temp_dir = create_test_fixture();

            std::fs::write(
                temp_dir.path().join("orders.view.yaml"),
                minimal_view_yaml("orders"),
            )
            .unwrap();
            std::fs::write(temp_dir.path().join("readme.txt"), "This is not a view").unwrap();
            std::fs::write(temp_dir.path().join("data.json"), "{}").unwrap();

            let config = ParserConfig::new(temp_dir.path()).with_validation(false);
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse();
            assert!(result.is_ok());
            let parse_result = result.unwrap();
            // Should only parse the .view.yaml file
            assert_eq!(parse_result.semantic_layer.views.len(), 1);
        }
    }

    mod parse_view_file_tests {
        use super::*;

        #[test]
        fn test_parse_valid_view_file() {
            let temp_dir = create_test_fixture();
            let view_path = temp_dir.path().join("test.view.yaml");
            std::fs::write(&view_path, minimal_view_yaml("test")).unwrap();

            let config = ParserConfig::new(temp_dir.path());
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse_view_file(&view_path);
            assert!(result.is_ok());
            let view = result.unwrap();
            assert_eq!(view.name, "test");
        }

        #[test]
        fn test_parse_view_file_not_found_fails() {
            let temp_dir = create_test_fixture();
            let config = ParserConfig::new(temp_dir.path());
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse_view_file(&PathBuf::from("/nonexistent/path.yaml"));
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                SemanticLayerError::IOError(_)
            ));
        }

        #[test]
        fn test_parse_malformed_yaml_fails() {
            let temp_dir = create_test_fixture();
            let view_path = temp_dir.path().join("bad.view.yaml");
            std::fs::write(&view_path, "this: is: not: valid: yaml: [").unwrap();

            let config = ParserConfig::new(temp_dir.path());
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse_view_file(&view_path);
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                SemanticLayerError::ParsingError(_)
            ));
        }
    }

    mod legacy_globals_rejection_tests {
        use super::*;

        #[test]
        fn test_view_with_inherits_from_is_rejected() {
            let temp_dir = create_test_fixture();
            let view_path = temp_dir.path().join("orders.view.yaml");
            std::fs::write(
                &view_path,
                r#"name: orders
inherits_from: globals.semantics.views.orders
table: orders
entities: []
dimensions: []
"#,
            )
            .unwrap();

            let parser = SemanticLayerParser::new(ParserConfig::new(temp_dir.path()));
            let err = parser.parse_view_file(&view_path).unwrap_err();
            assert!(matches!(err, SemanticLayerError::ParsingError(_)));
            assert!(format!("{err:?}").contains("inherits_from"));
        }

        #[test]
        fn test_view_with_globals_template_is_rejected() {
            let temp_dir = create_test_fixture();
            let view_path = temp_dir.path().join("orders.view.yaml");
            std::fs::write(
                &view_path,
                r#"name: orders
description: "{{ globals.semantics.descriptions.orders }}"
table: orders
entities: []
dimensions: []
"#,
            )
            .unwrap();

            let parser = SemanticLayerParser::new(ParserConfig::new(temp_dir.path()));
            let err = parser.parse_view_file(&view_path).unwrap_err();
            assert!(matches!(err, SemanticLayerError::ParsingError(_)));
        }

        #[test]
        fn test_topic_with_globals_template_is_rejected() {
            let temp_dir = create_test_fixture();
            let topic_path = temp_dir.path().join("sales.topic.yaml");
            std::fs::write(
                &topic_path,
                r#"name: sales
description: "{{globals.semantics.descriptions.sales}}"
views: []
"#,
            )
            .unwrap();

            let parser = SemanticLayerParser::new(ParserConfig::new(temp_dir.path()));
            let err = parser.parse_topic_file(&topic_path).unwrap_err();
            assert!(matches!(err, SemanticLayerError::ParsingError(_)));
        }
    }

    mod variable_extraction_tests {
        use super::*;

        #[test]
        fn test_parse_view_with_variables_succeeds() {
            let temp_dir = create_test_fixture();

            // View with variable references in table and dimension expressions
            let view_with_vars = r#"
name: orders
description: Test view with variables
table: "{{variables.schema}}.orders"
entities: []
dimensions:
  - name: filtered_amount
    expr: "CASE WHEN region = '{{variables.region}}' THEN amount ELSE 0 END"
    type: number
"#;
            std::fs::write(temp_dir.path().join("orders.view.yaml"), view_with_vars).unwrap();

            let config = ParserConfig::new(temp_dir.path()).with_validation(false);
            let parser = SemanticLayerParser::new(config);

            let result = parser.parse();
            assert!(result.is_ok());
            let parse_result = result.unwrap();

            // View should be parsed successfully
            assert_eq!(parse_result.semantic_layer.views.len(), 1);
            assert_eq!(parse_result.semantic_layer.views[0].name, "orders");

            // The parser should preserve variable references in the parsed view
            // Variables are stored as placeholders and extracted during parsing
            assert!(
                !parse_result.variables_found.is_empty(),
                "Should extract variables from view"
            );
        }
    }
}
