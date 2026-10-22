use serde_json::Value;

pub fn nt(v: &Value) -> &str {
    v.get("nodeType").and_then(Value::as_str).unwrap_or("")
}

pub fn id(v: &Value) -> Option<i64> {
    v.get("id").and_then(Value::as_i64)
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

pub fn refd(v: &Value) -> Option<i64> {
    v.get("referencedDeclaration").and_then(Value::as_i64)
}

pub fn flag(v: &Value, k: &str, default: bool) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(default)
}
