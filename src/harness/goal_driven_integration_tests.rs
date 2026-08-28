//! Integración Goal-Driven con AutonomousConstruction.

#[cfg(test)]
mod tests {
    use crate::harness::action_policy::ActionPolicy;
    use crate::harness::autonomous_construction::{
        AutonomousConstructionConfig, AutonomousConstructionSession, ConstructionStatus,
    };
    use crate::harness::context::AgentContext;
    use crate::harness::criterion::CriterionKind;
    use crate::harness::goal_driven::{
        GapDrivenAgent, Goal, GoalDrivenLoop, GoalDrivenStatus, GoalEvaluator, GoalStatus,
    };
    use crate::harness::live_session::build_validate_compile_harness_with_policy;
    use crate::harness::specification::{AcceptanceCriterion, Requirement, Specification};
    use crate::harness::tools::RepairDiagnosticTool;

    fn compile_spec() -> Specification {
        Specification::new("spec-gd-int", "El código debe compilar")
            .with_requirements(vec![Requirement::new("req-c", "compilar")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-compile", "compila", CriterionKind::Compile)
                    .satisfying([crate::harness::RequirementId::new("req-c")]),
            ])
    }

    #[test]
    fn goal_driven_session_reaches_satisfied_with_gap_agent() {
        let spec = compile_spec();
        let goal = Goal::from_specification(spec.clone());
        let config = AutonomousConstructionConfig::new(spec, 6)
            .with_initial_source("fn main() { println!(\"ok\"); }\n");

        let mut harness =
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy());
        harness.register_tool(Box::new(RepairDiagnosticTool));

        let mut agent = GapDrivenAgent::new("compilar");
        let mut loop_ = GoalDrivenLoop::with_defaults(config.max_iterations);
        let ctx = AgentContext::new(format!("goal:{}", goal.description()))
            .with_working_artifact(
                crate::harness::autonomous_construction::initial_artifact_from_plan(
                    goal.id().clone(),
                    &crate::harness::specification_planner::plan_specification(&goal.specification)
                        .expect("plan")
                        .plan,
                    config.artifact_name.clone(),
                ),
            )
            .with_evaluation_specification(goal.specification.clone());

        let result = loop_.run(&harness, &mut agent, &goal, ctx);
        assert!(
            result.is_goal_satisfied() || result.final_evaluation.status == GoalStatus::Satisfied,
            "status={:?} eval={:?}",
            result.status,
            result.final_evaluation.status
        );
    }

    #[test]
    fn autonomous_construction_config_compatible_with_goal() {
        let spec = compile_spec();
        let goal = Goal::from_specification(spec.clone());
        let config = AutonomousConstructionConfig::new(spec, 4).with_initial_source("fn main() {}");
        let eval = GoalEvaluator::new().evaluate(&goal, &[]);
        assert_eq!(eval.goal_id.as_str(), "spec-gd-int");
        assert_ne!(eval.status, GoalStatus::Satisfied);

        let construction = AutonomousConstructionSession::run_with_model_client(
            config,
            Box::new(crate::harness::model::MockModelClient::new()),
        );
        assert!(
            construction.status == ConstructionStatus::Completed
                || construction.status == ConstructionStatus::MaxIterations
        );
        let _ = eval;
    }

    #[test]
    fn goal_driven_escalation_status_distinct_from_satisfied() {
        let spec = Specification::new("spec-esc", "endpoint HTTP")
            .with_requirements(vec![Requirement::new("req", "health")])
            .with_acceptance_criteria(vec![
                AcceptanceCriterion::new("ac-u", "health 200", CriterionKind::Unknown)
                    .satisfying([crate::harness::RequirementId::new("req")]),
            ]);
        let goal = Goal::from_specification(spec.clone());
        let mut loop_ = GoalDrivenLoop::new(2, 1);
        let harness =
            build_validate_compile_harness_with_policy(ActionPolicy::default_session_policy());
        let mut agent = GapDrivenAgent::new("health");
        let ctx = AgentContext::new("esc")
            .with_working_code("fn main() {}")
            .with_evaluation_specification(spec);

        let result = loop_.run(&harness, &mut agent, &goal, ctx);
        assert_ne!(result.status, GoalDrivenStatus::GoalSatisfied);
    }
}
