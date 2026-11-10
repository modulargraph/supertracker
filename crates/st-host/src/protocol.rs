//! The bridge's wire format (docs/ARCHITECTURE.md, "The bridge").
//!
//! - Request, interface to host: `{"id": <number>, "command": "<name>", "payload": {...}}`
//! - Reply, host to interface: `{"id": <number>, "ok": true, "result": ...}` or
//!   `{"id": <number>, "ok": false, "error": "<message>"}`
//! - Event, host to interface: `{"type": "<name>", "payload": {...}}`
//!
//! Every message is one JSON text. Both transports carry these texts unchanged.

use serde_json::{json, Number, Value};

/// A request as the interface sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Echoed in the reply. The interface counts requests, so this is a number.
    pub id: Number,
    /// `area.verb`, for example `project.save`.
    pub command: String,
    /// The command's arguments. A request without a payload carries an empty object.
    pub payload: Value,
}

/// What a received text turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Request(Request),
    /// Not a usable request, but it names an id: the sender is waiting, so it gets an error reply.
    Rejected { id: Number, error: String },
    /// Nothing can be answered (not JSON, or no id). The reason is for the log.
    Unreadable(String),
}

/// Reads one text from the interface.
pub fn parse(text: &str) -> Incoming {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => return Incoming::Unreadable(format!("not JSON ({error})")),
    };
    let Value::Object(mut fields) = value else {
        return Incoming::Unreadable("not a JSON object".to_string());
    };
    let id = match fields.remove("id") {
        Some(Value::Number(id)) => id,
        Some(_) => return Incoming::Unreadable("the request id is not a number".to_string()),
        None => return Incoming::Unreadable("the request has no id".to_string()),
    };
    let command = match fields.remove("command") {
        Some(Value::String(command)) if !command.is_empty() => command,
        _ => {
            return Incoming::Rejected { id, error: "The request does not name a command.".to_string() };
        }
    };
    let payload = match fields.remove("payload") {
        None | Some(Value::Null) => Value::Object(Default::default()),
        Some(payload) => payload,
    };
    Incoming::Request(Request { id, command, payload })
}

/// The text of a successful reply.
pub fn reply_ok(id: &Number, result: &Value) -> String {
    json!({ "id": id, "ok": true, "result": result }).to_string()
}

/// The text of a failed reply.
pub fn reply_error(id: &Number, error: &str) -> String {
    json!({ "id": id, "ok": false, "error": error }).to_string()
}

/// The text of an event.
pub fn event(name: &str, payload: &Value) -> String {
    json!({ "type": name, "payload": payload }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_read_with_its_payload() {
        let parsed = parse(r#"{"id": 7, "command": "project.save", "payload": {"path": "/tmp/a"}}"#);
        assert_eq!(
            parsed,
            Incoming::Request(Request {
                id: 7.into(),
                command: "project.save".to_string(),
                payload: json!({"path": "/tmp/a"}),
            })
        );
    }

    #[test]
    fn a_missing_or_null_payload_is_an_empty_object() {
        for text in [r#"{"id":1,"command":"ready"}"#, r#"{"id":1,"command":"ready","payload":null}"#] {
            let Incoming::Request(request) = parse(text) else { panic!("{text} should be a request") };
            assert_eq!(request.payload, json!({}));
        }
    }

    #[test]
    fn a_request_without_a_command_is_rejected_with_its_id() {
        let parsed = parse(r#"{"id": 3, "payload": {}}"#);
        assert_eq!(parsed, Incoming::Rejected { id: 3.into(), error: "The request does not name a command.".to_string() });
    }

    #[test]
    fn texts_that_cannot_be_answered_are_unreadable() {
        for text in ["", "nonsense", "[1,2]", r#"{"command":"ready"}"#, r#"{"id":"one","command":"ready"}"#] {
            assert!(matches!(parse(text), Incoming::Unreadable(_)), "{text}");
        }
    }

    #[test]
    fn replies_and_events_have_the_documented_shape() {
        let ok: Value = serde_json::from_str(&reply_ok(&5.into(), &json!({"saved": true}))).unwrap();
        assert_eq!(ok, json!({"id": 5, "ok": true, "result": {"saved": true}}));
        let failed: Value = serde_json::from_str(&reply_error(&6.into(), "No.")).unwrap();
        assert_eq!(failed, json!({"id": 6, "ok": false, "error": "No."}));
        let event: Value = serde_json::from_str(&event("status", &json!({"playing": false}))).unwrap();
        assert_eq!(event, json!({"type": "status", "payload": {"playing": false}}));
    }
}
