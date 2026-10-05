//! Formal implication checks. Run through `scripts/run-symcc.sh` with cvc5 1.3.1.

#![allow(deprecated)]

use std::str::FromStr;

use cedar_policy::{Authorizer, Decision, PolicySet, Schema};
use cedar_policy_symcc::{CedarSymCompiler, SymEnv, WellTypedPolicies, solver::LocalSolver};

fn schema() -> Schema {
    Schema::from_cedarschema_str(include_str!("../cedar/schema.cedarschema"))
        .unwrap()
        .0
}

fn policies(source: &str) -> PolicySet {
    PolicySet::from_str(source).unwrap()
}

async fn implication(
    left: &PolicySet,
    right: &PolicySet,
    action: &str,
) -> (bool, Option<cedar_policy_symcc::Env>) {
    let schema = schema();
    let expected_action = format!(r#"Action::"{action}""#);
    let request_env = schema
        .request_envs()
        .find(|env| env.action().to_string() == expected_action)
        .expect("action request environment");
    let symbolic = SymEnv::new(&schema, &request_env).unwrap();
    let left = WellTypedPolicies::from_policies(left, &request_env, &schema).unwrap();
    let right = WellTypedPolicies::from_policies(right, &request_env, &schema).unwrap();
    let mut compiler = CedarSymCompiler::new(LocalSolver::cvc5().unwrap()).unwrap();
    let implies = compiler
        .check_implies(&left, &right, &symbolic)
        .await
        .unwrap();
    let counterexample = compiler
        .check_implies_with_counterexample(&left, &right, &symbolic)
        .await
        .unwrap();
    (implies, counterexample)
}

#[tokio::test]
async fn live_policy_implies_reference_r() {
    let live = policies(include_str!("../cedar/live.cedar"));
    let reference = policies(include_str!("../cedar/reference_r.cedar"));
    for action in ["use", "disclose"] {
        let (implies, counterexample) = implication(&live, &reference, action).await;
        assert!(implies);
        assert!(counterexample.is_none());
        println!("check_implies live => R action={action}: true; counterexample: none");
    }
}

#[tokio::test]
async fn bad_disclose_permit_fails_with_counterexample() {
    let bad = policies(include_str!("../cedar/bad_permit.cedar"));
    let reference = policies(include_str!("../cedar/reference_r.cedar"));
    let (implies, counterexample) = implication(&bad, &reference, "disclose").await;
    assert!(!implies);
    let counterexample = counterexample.expect("unsafe permit must have a counterexample");
    let bad_result =
        Authorizer::new().is_authorized(&counterexample.request, &bad, &counterexample.entities);
    let reference_result = Authorizer::new().is_authorized(
        &counterexample.request,
        &reference,
        &counterexample.entities,
    );
    assert_eq!(bad_result.decision(), Decision::Allow);
    assert_eq!(reference_result.decision(), Decision::Deny);
    println!("check_implies bad => R: false");
    println!("counterexample request: {:?}", counterexample.request);
}
