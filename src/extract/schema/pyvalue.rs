//! Python's view of decoded JSON values, which v4's coercions go through.

use serde_json::{Number, Value};

use crate::domain::pytext::repr;

/// `type(value).__name__` after `json.loads`.
pub(super) fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `str(value)`. Object keys come out sorted, where Python keeps document order.
pub(super) fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => py_repr(other),
    }
}

fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => number_repr(number),
        Value::String(text) => repr(text),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(map) => {
            let items: Vec<String> = map
                .iter()
                .map(|(key, item)| format!("{}: {}", repr(key), py_repr(item)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

fn number_repr(number: &Number) -> String {
    match number.as_f64() {
        Some(float) if number.is_f64() => float_repr(float),
        _ => number.to_string(),
    }
}

/// Python `repr(float)`: shortest round trip, `1e+20` style exponents.
pub(super) fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let debug = format!("{value:?}");
    match debug.split_once('e') {
        Some((mantissa, exponent)) => {
            let (sign, digits) = match exponent.strip_prefix('-') {
                Some(digits) => ('-', digits),
                None => ('+', exponent),
            };
            let mantissa = mantissa.strip_suffix(".0").unwrap_or(mantissa);
            format!("{mantissa}e{sign}{digits:0>2}")
        }
        None => debug,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn values_render_as_python_str() {
        assert_eq!(py_str(&json!({})), "{}");
        assert_eq!(py_str(&json!(7)), "7");
        assert_eq!(py_str(&json!(82.0)), "82.0");
        assert_eq!(py_str(&json!(1e20)), "1e+20");
        assert_eq!(py_str(&json!(1.5e-7)), "1.5e-07");
        assert_eq!(py_str(&json!(["a", null, true])), "['a', None, True]");
    }
}
