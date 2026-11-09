use oxy_experiment::engine_in::{RatioIn, Refused};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

/// An engine result kept both ways: `raw` is what the engine serialised (stored in
/// the results row for audit), `view` is the mirror the report and patch rules read.
#[derive(Debug, Clone)]
pub struct Judged<T> {
    pub raw: Value,
    pub view: T,
}

impl<T: DeserializeOwned + Refused> Judged<T> {
    /// Serialise the engine's struct and read it back as the mirror. Neither step
    /// can fail for the engine's own types, but a refusal is the only honest
    /// answer if one ever does.
    pub fn from_engine<S: Serialize>(engine: &S) -> Self {
        let raw = match serde_json::to_value(engine) {
            Ok(raw) => raw,
            Err(e) => {
                return Self::refused(format!("the engine's result could not be serialised: {e}"));
            }
        };
        match serde_json::from_value::<T>(raw.clone()) {
            Ok(view) => Judged { raw, view },
            Err(e) => Self::refused(format!("the engine's result could not be read: {e}")),
        }
    }

    pub fn refused(reason: String) -> Self {
        Judged {
            raw: json!({ "refusal": &reason }),
            view: T::refused(reason),
        }
    }
}

const DRIVER_PHRASE: &str = "the driver";

/// "the driver" → "the driver `<measure>`", once: a message already carrying the
/// backticked name (or worded by the host, which names its measure) is left alone.
fn name_in(msg: &str, driver: &str) -> String {
    if msg.contains("the driver `") {
        return msg.to_string();
    }
    msg.replace(DRIVER_PHRASE, &format!("{DRIVER_PHRASE} `{driver}`"))
}

/// `estimate_ratio` does not know measure names (C2b), so any refusal of it that
/// says "the driver" is completed here, in the mirror and in the raw JSON.
pub fn name_the_driver(mut j: Judged<RatioIn>, driver: &str) -> Judged<RatioIn> {
    for slot in [&mut j.view.refusal, &mut j.view.first_stage.refusal] {
        if let Some(m) = slot {
            *m = name_in(m, driver);
        }
    }
    for path in ["/refusal", "/first_stage/refusal", "/target_effect/refusal"] {
        if let Some(Value::String(m)) = j.raw.pointer_mut(path) {
            *m = name_in(m, driver);
        }
    }
    j
}
