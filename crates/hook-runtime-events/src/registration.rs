use crate::normalise;
use iii_sdk::{III, RegisterFunction};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NormaliseInput {
    pub agent: String,
    pub raw: Value,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DetectInput {
    pub raw: Value,
}

pub fn register(iii: &III) {
    iii.register_function(RegisterFunction::new(
        "hook::runtime_events::normalise",
        |input: NormaliseInput| -> Result<Value, String> {
            normalise::normalise(&input.agent, input.raw).map_err(|e| e.to_string())
        },
    ));

    iii.register_function(RegisterFunction::new(
        "hook::runtime_events::detect",
        |input: DetectInput| -> Result<Value, String> {
            Ok(json!({
                "match": normalise::matches_shape(&input.raw),
                "shape": "runtime_events",
            }))
        },
    ));
}
