//!

use mini_v8::MiniV8;

///
pub fn from_value(mv8: &MiniV8, item: serde_json::Value) -> mini_v8::Result<mini_v8::Value> {
    let result = match item {
        serde_json::Value::Null => mini_v8::Value::Null,
        serde_json::Value::Bool(val) => mini_v8::Value::Boolean(val),
        serde_json::Value::Number(val) => {
            if let Some(val) = val.as_f64() {
                mini_v8::Value::Number(val)
            } else {
                mini_v8::Value::Undefined
            }
        }
        serde_json::Value::String(val) => mini_v8::Value::String(mv8.create_string(&val)),
        serde_json::Value::Array(val) => {
            let items = mv8.create_array();

            for item in val {
                items.push(from_value(mv8, item)?)?;
            }

            mini_v8::Value::Array(items)
        }
        serde_json::Value::Object(val) => {
            let object = mv8.create_object();

            for (key, value) in val {
                object.set(key, from_value(mv8, value)?)?;
            }

            mini_v8::Value::Object(object)
        }
    };

    Ok(result)
}

///
pub fn from_v8(item: mini_v8::Value) -> mini_v8::Result<serde_json::Value> {
    let result = match item {
        mini_v8::Value::Undefined | mini_v8::Value::Null => serde_json::Value::Null,
        mini_v8::Value::Boolean(val) => serde_json::Value::Bool(val),
        mini_v8::Value::Number(val) => {
            if let Some(num) = serde_json::Number::from_f64(val) {
                serde_json::Value::Number(num)
            } else {
                serde_json::Value::Null
            }
        }
        mini_v8::Value::String(val) => serde_json::Value::String(val.to_string()),
        mini_v8::Value::Array(val) => {
            let items: mini_v8::Result<Vec<_>> =
                val.elements().map(|item| from_v8(item?)).collect();
            let items = items?;

            serde_json::Value::Array(items)
        }
        mini_v8::Value::Object(val) => {
            let props: mini_v8::Result<serde_json::Map<String, serde_json::Value>> = val
                .properties(false)?
                .map(|prop| {
                    let (key, value) = prop?;
                    Ok((key, from_v8(value)?))
                })
                .collect();
            let props = props?;

            serde_json::Value::Object(props)
        }
        mini_v8::Value::Function(_) | mini_v8::Value::Date(_) => {
            serde_json::Value::Object(serde_json::Map::new())
        }
    };

    Ok(result)
}
