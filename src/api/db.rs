//! RAGFlow `api/db/__init__.py` value contracts.
//!
//! The Python module is the shared vocabulary for persisted/API file kinds,
//! tenant roles and permissions, connector input modes, Canvas categories and
//! pipeline task groups.  RayRAG keeps those values typed while re-exporting
//! definitions already owned by the corresponding Rust service modules.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use crate::api::common::TeamPermission as TenantPermission;
pub use crate::api::joint_services::UserTenantRole;
pub use crate::api::utils::FileType;
pub use crate::common::constants::{PipelineTaskType, VALID_PIPELINE_TASK_TYPES};

/// `SerializedType(IntEnum)` — the persisted discriminator used by RAGFlow's
/// binary/JSON serialized database fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SerializedType {
    Pickle = 1,
    Json = 2,
}

impl SerializedType {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for SerializedType {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Pickle),
            2 => Ok(Self::Json),
            _ => Err("serialized type must be 1 (pickle) or 2 (json)"),
        }
    }
}

impl Serialize for SerializedType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(self.as_u8())
    }
}

impl<'de> Deserialize<'de> for SerializedType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(u8::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Connector ingestion trigger mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    LoadState,
    Poll,
    Event,
    SlimRetrieval,
}

impl InputType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LoadState => "load_state",
            Self::Poll => "poll",
            Self::Event => "event",
            Self::SlimRetrieval => "slim_retrieval",
        }
    }
}

/// Canvas storage category. Variant spelling follows the upstream public API;
/// wire values remain `agent_canvas` and `dataflow_canvas`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CanvasCategory {
    #[serde(rename = "agent_canvas")]
    Agent,
    #[serde(rename = "dataflow_canvas")]
    DataFlow,
}

impl CanvasCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent_canvas",
            Self::DataFlow => "dataflow_canvas",
        }
    }
}

impl std::str::FromStr for CanvasCategory {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "agent_canvas" => Ok(Self::Agent),
            "dataflow_canvas" => Ok(Self::DataFlow),
            _ => Err("canvas category must be 'agent_canvas' or 'dataflow_canvas'"),
        }
    }
}

/// Every database-level file kind accepted by the fixed upstream API.
pub const VALID_FILE_TYPES: [FileType; 7] = [
    FileType::Pdf,
    FileType::Doc,
    FileType::Visual,
    FileType::Aural,
    FileType::Virtual,
    FileType::Folder,
    FileType::Other,
];

/// Lowercase task values whose KB-level fan-out progress is frozen until the
/// special task completes. `parse`, `download` and `memory` are excluded.
pub const PIPELINE_SPECIAL_PROGRESS_FREEZE_TASK_TYPES: [&str; 5] =
    ["raptor", "graphrag", "mindmap", "artifact", "skill"];

pub const KNOWLEDGEBASE_FOLDER_NAME: &str = ".knowledgebase";
pub const SKILLS_FOLDER_NAME: &str = "skills";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_type_keeps_int_enum_wire_values() {
        assert_eq!(SerializedType::Pickle.as_u8(), 1);
        assert_eq!(SerializedType::try_from(2), Ok(SerializedType::Json));
        assert!(SerializedType::try_from(0).is_err());
        assert_eq!(serde_json::to_value(SerializedType::Json).unwrap(), 2);
        assert_eq!(
            serde_json::from_value::<SerializedType>(serde_json::json!(1)).unwrap(),
            SerializedType::Pickle
        );
    }

    #[test]
    fn string_enums_match_fixed_ragflow_values() {
        assert_eq!(
            serde_json::to_value(InputType::LoadState).unwrap(),
            "load_state"
        );
        assert_eq!(InputType::SlimRetrieval.as_str(), "slim_retrieval");
        assert_eq!(
            serde_json::to_value(CanvasCategory::Agent).unwrap(),
            "agent_canvas"
        );
        assert_eq!(CanvasCategory::DataFlow.as_str(), "dataflow_canvas");
        assert_eq!(
            "dataflow_canvas".parse::<CanvasCategory>().unwrap(),
            CanvasCategory::DataFlow
        );
        assert!("ingestion".parse::<CanvasCategory>().is_err());
        assert_eq!(TenantPermission::Team.as_str(), "team");
        assert_eq!(UserTenantRole::Invite.as_str(), "invite");
    }

    #[test]
    fn file_and_pipeline_sets_match_fixed_membership() {
        assert_eq!(VALID_FILE_TYPES.len(), 7);
        assert!(VALID_FILE_TYPES.contains(&FileType::Virtual));
        assert!(VALID_FILE_TYPES.contains(&FileType::Folder));
        assert_eq!(VALID_PIPELINE_TASK_TYPES.len(), 7);
        assert!(VALID_PIPELINE_TASK_TYPES.contains(&PipelineTaskType::Artifact));
        assert!(VALID_PIPELINE_TASK_TYPES.contains(&PipelineTaskType::Skill));
        assert!(!VALID_PIPELINE_TASK_TYPES.contains(&PipelineTaskType::Memory));
        assert_eq!(
            PIPELINE_SPECIAL_PROGRESS_FREEZE_TASK_TYPES,
            ["raptor", "graphrag", "mindmap", "artifact", "skill"]
        );
        assert_eq!(KNOWLEDGEBASE_FOLDER_NAME, ".knowledgebase");
        assert_eq!(SKILLS_FOLDER_NAME, "skills");
    }
}
