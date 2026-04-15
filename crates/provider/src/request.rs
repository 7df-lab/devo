use serde_json::Value;

/// Merges an extra JSON object into a provider request body.
pub fn merge_extra_body(body: &mut Value, extra_body: Option<&Value>) {
    let Some(extra_body) = extra_body else {
        return;
    };
    let Some(body_object) = body.as_object_mut() else {
        return;
    };
    let Some(extra_object) = extra_body.as_object() else {
        return;
    };

    for (key, value) in extra_object {
        body_object.insert(key.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::merge_extra_body;
    use serde_json::json;

    #[test]
    fn merge_extra_body_overrides_existing_fields() {
        let mut body = json!({
            "model": "base-model",
            "temperature": 0.2
        });
        let extra = json!({
            "temperature": 0.8,
            "top_k": 32
        });

        merge_extra_body(&mut body, Some(&extra));

        assert_eq!(
            body,
            json!({
                "model": "base-model",
                "temperature": 0.8,
                "top_k": 32
            })
        );
    }
}
