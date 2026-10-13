//! Product teaching tools: a Windows runtime session and plain output files.
use crate::{Platform, SchemaMode, ToolAnnotations, ToolContract};
use serde_json::json;

pub fn contracts() -> Vec<ToolContract> {
    ["start_teaching", "get_teaching_state", "stop_teaching"]
        .into_iter()
        .map(|name| {
            let mut properties = json!({
                "session_id": {"type":"string","minLength":1,"maxLength":128},
                "session": {"type":"string","description":"Use one explicit session label for teaching across separate CLI calls."},
                "after_sequence": {"type":"integer","minimum":0},
                "timeline_limit": {"type":"integer","minimum":1,"maximum":1000},
                "after_evidence_sequence": {"type":"integer","minimum":0},
                "evidence_limit": {"type":"integer","minimum":1,"maximum":1000}
            });
            if name == "start_teaching" {
                properties["output_dir"] = json!({"type":"string","description":"Parent directory. A fresh teaching-UUID subdirectory is created; existing files are never overwritten."});
                properties["max_seconds"] = json!({"type":"integer","minimum":1,"maximum":600,"default":600});
                properties["exclude_pids"] = json!({"type":"array","maxItems":32,"items":{"type":"integer","minimum":1}});
            }
            if name == "stop_teaching" {
                properties["cancel"] = json!({"type":"boolean","default":false,"description":"Stop and remove only files created by this session."});
            }
            ToolContract {
                name: name.into(),
                description: format!("{name}: Windows human demonstration. One active session per runtime; automation is mutually exclusive. Password/unknown fields omit text and images. Results include action sequences, output_dir and relative file references. OCR is used only when committed text is unavailable. complete describes action collection/storage; evidence.complete separately describes optional screenshots/OCR. Normal stop writes a completion marker when actions are complete; cancel cleans only this session. CLI callers must use an explicit session label."),
                platforms: vec![Platform::Windows],
                aliases: vec![],
                capabilities: vec![],
                annotations: ToolAnnotations {
                    read_only: name == "get_teaching_state",
                    destructive: name == "stop_teaching",
                    idempotent: name != "start_teaching",
                    open_world: false,
                },
                schema_mode: SchemaMode::CanonicalRuntime,
                cursor_semantics: None,
                input_schema: json!({
                    "type":"object", "properties":properties,
                    "required":["session_id"], "additionalProperties":false
                }),
                success_output_schema: None,
                error_output_schema: None,
                output_validator: crate::default_output_validator(),
            }
        })
        .collect()
}
