use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, NaiveDate, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

// --- Enums ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPermission {
    Viewer,
    Editor,
    Admin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectView {
    Board,
    List,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Blocked,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NthWeek {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RecurrenceRule {
    Daily,
    Weekly { days: Vec<u8> },
    Monthly { day_of_month: u8 },
    /// `weekday` uses the same 0=Mon..6=Sun encoding as `Weekly { days }`.
    MonthlyByWeekday { nth: NthWeek, weekday: u8 },
    Yearly { month: u8, day: u8 },
    Custom { interval_days: u32 },
}

// --- Serde helper for Option<NaiveDate> as "YYYY-MM-DD" string ---

pub mod naive_date_option {
    use chrono::NaiveDate;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(date: &Option<NaiveDate>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match date {
            Some(d) => serializer.serialize_str(&d.format("%Y-%m-%d").to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<NaiveDate>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<String> = Option::deserialize(deserializer)?;
        match s {
            Some(ref s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

// --- Documents ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMember {
    pub user_id: ObjectId,
    pub username: String,
    pub permission: ProjectPermission,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProject {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub description: Option<String>,
    pub color: String,
    pub icon: Option<String>,
    pub owner_id: ObjectId,
    pub members: Vec<ProjectMember>,
    pub default_view: ProjectView,
    #[serde(default)]
    pub archived: bool,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSection {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub project_id: ObjectId,
    pub name: String,
    pub position: i32,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub project_id: ObjectId,
    pub section_id: Option<ObjectId>,
    pub parent_task_id: Option<ObjectId>,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub status_note: Option<String>,
    pub priority: Option<TaskPriority>,
    pub assignee_id: Option<ObjectId>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, with = "naive_date_option")]
    pub due_date: Option<NaiveDate>,
    pub recurrence_rule: Option<RecurrenceRule>,
    pub position: i32,
    #[serde(default)]
    pub attachments: Vec<ObjectId>,
    pub created_by: ObjectId,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskComment {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub task_id: ObjectId,
    pub project_id: ObjectId,
    pub author_id: ObjectId,
    pub body: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskLabel {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub project_id: ObjectId,
    pub name: String,
    pub color: String,
}
