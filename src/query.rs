use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

pub fn item_groups<'a>(document: &'a Value, name: &str) -> Result<&'a [Value]> {
    let items = document
        .get("Items")
        .and_then(Value::as_object)
        .context("MSBuild query JSON has no 'Items' object")?;
    match get_case_insensitive(items, name) {
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .with_context(|| format!("MSBuild item group '{name}' is not an array")),
        None => Ok(&[]),
    }
}

pub fn target_items<'a>(document: &'a Value, target: &str) -> Result<&'a [Value]> {
    let targets = document
        .get("TargetResults")
        .and_then(Value::as_object)
        .context("MSBuild query JSON has no 'TargetResults' object")?;
    let target_result = get_case_insensitive(targets, target)
        .with_context(|| format!("MSBuild query JSON has no result for target '{target}'"))?;
    let result = metadata(target_result, "Result").unwrap_or_default();
    if !result.eq_ignore_ascii_case("Success") {
        bail!("MSBuild target '{target}' reported result '{result}'");
    }
    target_result
        .get("Items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .with_context(|| format!("MSBuild target '{target}' has no item array"))
}

pub fn metadata<'a>(item: &'a Value, name: &str) -> Option<&'a str> {
    item.as_object()
        .and_then(|object| get_case_insensitive(object, name))
        .and_then(Value::as_str)
}

pub fn property<'a>(document: &'a Value, name: &str) -> Option<&'a str> {
    document
        .get("Properties")
        .and_then(Value::as_object)
        .and_then(|object| get_case_insensitive(object, name))
        .and_then(Value::as_str)
}

fn get_case_insensitive<'a>(object: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    object
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}
