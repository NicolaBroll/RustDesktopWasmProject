use crate::CoreError;
use serde_json::Value;
use std::str::FromStr;

/// Invocazione testuale di un comando: `id` oppure `id=<json>`.
///
/// È il formato usato dalla CLI (`run --cmd`, `eval`) e dai test.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub id: String,
    pub params: Value,
}

impl FromStr for Invocation {
    type Err = CoreError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        let (id, params) = match input.split_once('=') {
            Some((id, json)) => (id.trim(), Some(json.trim())),
            None => (input, None),
        };
        if !is_valid_command_id(id) {
            return Err(CoreError::InvalidCommandId(id.to_owned()));
        }
        let params = match params {
            None | Some("") => Value::Object(Default::default()),
            Some(json) => serde_json::from_str(json).map_err(|e| CoreError::InvalidParams {
                id: id.to_owned(),
                reason: e.to_string(),
            })?,
        };
        Ok(Self {
            id: id.to_owned(),
            params,
        })
    }
}

/// Un id di comando è non vuoto e usa solo `a-z`, `0-9`, `.`, `_`, `-`.
pub fn is_valid_command_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn id_only_gets_empty_object() {
        let inv: Invocation = "edit.undo".parse().unwrap();
        assert_eq!(inv.id, "edit.undo");
        assert_eq!(inv.params, json!({}));
    }

    #[test]
    fn id_with_json() {
        let inv: Invocation = r#" repo.open = {"path": "a=b"} "#.parse().unwrap();
        assert_eq!(inv.id, "repo.open");
        assert_eq!(inv.params, json!({"path": "a=b"}));
    }

    #[test]
    fn rejects_bad_id() {
        assert!(matches!(
            "Repo.Open".parse::<Invocation>(),
            Err(CoreError::InvalidCommandId(_))
        ));
        assert!(matches!(
            "={}".parse::<Invocation>(),
            Err(CoreError::InvalidCommandId(_))
        ));
    }

    #[test]
    fn rejects_bad_json() {
        assert!(matches!(
            "repo.open={path".parse::<Invocation>(),
            Err(CoreError::InvalidParams { .. })
        ));
    }
}
