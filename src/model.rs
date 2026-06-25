//! The data model handed back to the caller (serialized as JSON).
//! Deliberately small: just enough to drive a Gantt / WBS in Typst.

use serde::Serialize;

#[derive(Serialize, Default)]
pub struct Project {
    pub format: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<Resource>,
    pub tasks: Vec<Task>,
}

#[derive(Serialize, Default)]
pub struct Resource {
    pub unique_id: i32,
    pub id: i32,
    pub name: String,
}

#[derive(Serialize, Default)]
pub struct Task {
    pub unique_id: i32,
    pub id: i32,
    pub name: String,
    /// 1-based hierarchy depth; reconstructs the work-breakdown tree.
    pub outline_level: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_unique_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>, // ISO-8601
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish: Option<String>, // ISO-8601
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_finish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_finish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub early_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub early_finish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub late_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub late_finish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraint_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_finish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_hours: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_cost: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resource_names: Vec<String>,
    pub percent_complete: u16,
}
