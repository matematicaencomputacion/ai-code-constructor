use crate::planner::BuildPlan;

#[derive(Debug, Clone)]
pub struct CodeState {
    pub request: String,
    pub plan: Option<BuildPlan>,
    pub code: Option<String>,
    pub errors: Vec<String>,
    pub feedback: Vec<String>,
    pub iteration: u32,
}
