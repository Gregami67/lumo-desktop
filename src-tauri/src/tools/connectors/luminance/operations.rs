use super::super::super::connector_state::ToolPolicy;
use super::super::super::error::ToolError;
use super::super::super::registry::ToolDefinition;
use base64::Engine;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

#[derive(Debug, Deserialize)]
struct OperationTable {
    operations: Vec<Operation>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Operation {
    pub(super) name: String,
    title: String,
    description: String,
    method: String,
    path: String,
    params: Vec<Param>,
    #[serde(default)]
    bulk: bool,
    body: Option<Body>,
    pub(super) response: ResponseKind,
}

#[derive(Debug, Deserialize)]
pub(super) struct Param {
    name: String,
    #[serde(rename = "in")]
    location: ParamLocation,
    required: bool,
    description: Option<String>,
    schema: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ParamLocation {
    Path,
    Query,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Body {
    Json { required: bool, schema: Value },
    Binary { required: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResponseKind {
    Json,
    Binary,
}

/// Generated from Luminance's OpenAPI spec by `scripts/generate-luminance-operations.mjs`.
pub(super) static OPERATIONS: LazyLock<Vec<Operation>> = LazyLock::new(|| {
    serde_json::from_str::<OperationTable>(include_str!("operations.json"))
        .expect("operations.json is valid")
        .operations
});

#[derive(Debug, PartialEq)]
pub(super) enum RequestBody {
    None,
    Json(Value),
    Binary(Vec<u8>),
}

#[derive(Debug)]
pub(super) struct ApiRequest {
    pub(super) method: Method,
    pub(super) path: String,
    pub(super) query: Vec<(String, String)>,
    pub(super) body: RequestBody,
}

pub(super) fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub(super) fn push_query(
    query: &mut Vec<(String, String)>,
    name: &str,
    value: &Value,
) -> Result<(), ToolError> {
    match value {
        Value::Null => {}
        Value::Array(items) => {
            for item in items {
                let item = scalar_string(item).ok_or_else(|| {
                    ToolError::InvalidInput(format!(
                        "{name} must hold strings, numbers or booleans"
                    ))
                })?;
                query.push((name.to_string(), item));
            }
        }
        Value::Object(_) => query.push((name.to_string(), value.to_string())),
        other => {
            let item = scalar_string(other).ok_or_else(|| {
                ToolError::InvalidInput(format!("{name} must be a string, number or boolean"))
            })?;
            query.push((name.to_string(), item));
        }
    }
    Ok(())
}

pub(super) fn build_request(operation: &Operation, input: &Value) -> Result<ApiRequest, ToolError> {
    let empty = Map::new();
    let args = match input {
        Value::Object(map) => map,
        Value::Null => &empty,
        _ => {
            return Err(ToolError::InvalidInput(
                "arguments must be an object".into(),
            ))
        }
    };
    let mut path = operation.path.clone();
    let mut query = Vec::new();
    for param in &operation.params {
        let value = args.get(&param.name).filter(|v| !v.is_null());
        match (param.location, value) {
            (ParamLocation::Path, Some(value)) => {
                let segment = scalar_string(value)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ToolError::InvalidInput(format!(
                            "{} must be a string or number",
                            param.name
                        ))
                    })?;
                let encoded: String =
                    url::form_urlencoded::byte_serialize(segment.as_bytes()).collect();
                path = path.replace(&format!("{{{}}}", param.name), &encoded);
            }
            (ParamLocation::Query, Some(value)) => push_query(&mut query, &param.name, value)?,
            (_, None) if param.required => {
                return Err(ToolError::InvalidInput(format!(
                    "{} is required",
                    param.name
                )))
            }
            (_, None) => {}
        }
    }
    if operation.bulk {
        if let Some(filter) = args.get("filter").filter(|v| !v.is_null()) {
            let filter = filter.as_object().ok_or_else(|| {
                ToolError::InvalidInput("filter must be an object of field: value".into())
            })?;
            for (name, value) in filter {
                push_query(&mut query, name, value)?;
            }
        }
        if query.is_empty() {
            return Err(ToolError::InvalidInput(format!(
                "{} changes every record its filters match, so at least one filter is required",
                operation.name
            )));
        }
    }
    let body = match &operation.body {
        None => RequestBody::None,
        Some(Body::Json { required, .. }) => match args.get("body").filter(|v| !v.is_null()) {
            Some(value) => RequestBody::Json(value.clone()),
            None if *required => return Err(ToolError::InvalidInput("body is required".into())),
            None => RequestBody::None,
        },
        Some(Body::Binary { required }) => match args.get("content_base64").and_then(Value::as_str)
        {
            Some(encoded) => RequestBody::Binary(
                base64::engine::general_purpose::STANDARD
                    .decode(encoded.trim())
                    .map_err(|e| {
                        ToolError::InvalidInput(format!("content_base64 is not base64: {e}"))
                    })?,
            ),
            None if *required => {
                return Err(ToolError::InvalidInput("content_base64 is required".into()))
            }
            None => RequestBody::None,
        },
    };
    let method = Method::from_bytes(operation.method.as_bytes())
        .map_err(|e| ToolError::ExecutionFailed(format!("invalid method: {e}")))?;
    Ok(ApiRequest {
        method,
        path,
        query,
        body,
    })
}

pub(super) fn input_schema(operation: &Operation) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for param in &operation.params {
        let mut schema = param.schema.clone();
        if let (Some(description), Some(object)) = (&param.description, schema.as_object_mut()) {
            object.insert("description".into(), json!(description));
        }
        properties.insert(param.name.clone(), schema);
        if param.required {
            required.push(json!(param.name));
        }
    }
    match &operation.body {
        Some(Body::Json {
            required: needed,
            schema,
        }) => {
            properties.insert("body".into(), schema.clone());
            if *needed {
                required.push(json!("body"));
            }
        }
        Some(Body::Binary { required: needed }) => {
            properties.insert(
                "content_base64".into(),
                json!({ "type": "string", "description": "File contents, base64-encoded." }),
            );
            if *needed {
                required.push(json!("content_base64"));
            }
        }
        None => {}
    }
    if operation.bulk {
        properties.insert(
            "filter".into(),
            json!({
                "type": "object",
                "description": "Extra query filters as field: value. Every matching record is affected, and at least one filter (here or above) is required.",
                "additionalProperties": { "type": ["string", "number", "boolean"] }
            }),
        );
    }
    json!({ "type": "object", "properties": properties, "required": required })
}

pub(super) fn tool_definition(operation: &Operation) -> ToolDefinition {
    let mut description = operation.description.clone();
    if operation.bulk {
        description.push_str("\n\nAffects every record matching the filters.");
    }
    if operation.response == ResponseKind::Binary {
        description
            .push_str("\n\nReturns the file as UTF-8 text when it is textual, otherwise base64.");
    }
    ToolDefinition {
        name: operation.name.clone(),
        title: Some(operation.title.clone()),
        description,
        input_schema: input_schema(operation),
        policy: if operation.method == "GET" {
            ToolPolicy::AutoApprove
        } else {
            ToolPolicy::Ask
        },
        read_only: Some(operation.method == "GET"),
        open_world: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn operation(name: &str) -> &'static Operation {
        OPERATIONS
            .iter()
            .find(|op| op.name == name)
            .expect("operation exists")
    }

    #[test]
    fn every_operation_becomes_a_uniquely_named_tool() {
        let names: HashSet<_> = OPERATIONS.iter().map(|op| op.name.as_str()).collect();
        assert_eq!(names.len(), OPERATIONS.len());
        assert!(OPERATIONS.len() > 100);
        assert!(OPERATIONS.iter().all(|op| op.name.len() <= 60));
    }

    #[test]
    fn only_reads_run_without_asking() {
        for op in OPERATIONS.iter() {
            let expected = if op.method == "GET" {
                ToolPolicy::AutoApprove
            } else {
                ToolPolicy::Ask
            };
            assert_eq!(tool_definition(op).policy, expected, "{}", op.name);
        }
    }

    #[test]
    fn path_and_query_parameters_are_placed_and_encoded() {
        let request = build_request(
            operation("list_documents_in_project"),
            &json!({ "project_id": 7, "name": "a/b c", "folder_id": 3 }),
        )
        .unwrap();
        assert_eq!(request.method, Method::GET);
        assert_eq!(request.path, "/api2/projects/7/documents");
        assert!(request.query.contains(&("name".into(), "a/b c".into())));
        assert!(request.query.contains(&("folder_id".into(), "3".into())));

        let request = build_request(
            operation("get_document"),
            &json!({ "project_id": 1, "document_id": "../x" }),
        )
        .unwrap();
        assert_eq!(request.path, "/api2/projects/1/documents/..%2Fx");
    }

    #[test]
    fn a_missing_path_parameter_is_refused() {
        let err =
            build_request(operation("get_document"), &json!({ "project_id": 1 })).unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidInput(ref m) if m.contains("document_id")),
            "{err}"
        );
    }

    #[test]
    fn a_bulk_change_without_any_filter_is_refused() {
        let op = operation("delete_matters_by_filter");
        assert!(op.bulk);
        let err = build_request(op, &json!({ "project_id": 1 })).unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidInput(ref m) if m.contains("filter")),
            "{err}"
        );

        let request =
            build_request(op, &json!({ "project_id": 1, "filter": { "id": 5 } })).unwrap();
        assert_eq!(request.query, vec![("id".to_string(), "5".to_string())]);

        let documented = build_request(
            operation("delete_documents_by_filter"),
            &json!({ "project_id": 1, "id": 9 }),
        )
        .unwrap();
        assert_eq!(documented.query, vec![("id".to_string(), "9".to_string())]);
    }

    #[test]
    fn json_and_binary_bodies_are_taken_from_their_own_fields() {
        let request = build_request(
            operation("create_matter_and_optional_annotations_and_version"),
            &json!({ "project_id": 1, "body": { "name": "NDA" } }),
        )
        .unwrap();
        assert_eq!(request.body, RequestBody::Json(json!({ "name": "NDA" })));

        let request = build_request(
            operation("upload_document_into_folder"),
            &json!({ "project_id": 1, "folder_id": 2, "name": "a.txt", "content_base64": "aGk=" }),
        )
        .unwrap();
        assert_eq!(request.body, RequestBody::Binary(b"hi".to_vec()));
    }
}
