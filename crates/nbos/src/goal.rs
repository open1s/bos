//! Durable working goals and the store a turn's goal tools write to.

use crate::utils::json_to_py;
use agent::goal::{Goal as CoreGoal, GoalState, MAX_OBJECTIVE, MAX_REASON};
use pyo3::prelude::*;

fn state_name(state: GoalState) -> &'static str {
    match state {
        GoalState::Active => "active",
        GoalState::Paused => "paused",
        GoalState::Completed => "completed",
        GoalState::Blocked => "blocked",
    }
}

fn goal_json(goal: &CoreGoal) -> serde_json::Value {
    serde_json::json!({
        "objective": goal.objective,
        "state": state_name(goal.state),
        "rounds": goal.rounds,
        "max_rounds": goal.max_rounds,
        "blocked_reason": goal.blocked_reason.as_ref().map(|reason| serde_json::json!({
            "code": reason.code,
            "message": reason.message,
        })),
        "activation": goal.activation.as_str(),
    })
}

/// A durable working goal.
///
/// Every field is sanitized on construction, so a host can pass whatever a
/// person typed and still get a value every other host can render.
#[pyclass(name = "Goal", skip_from_py_object)]
#[derive(Clone)]
pub struct PyGoal {
    pub(crate) inner: CoreGoal,
}

#[pymethods]
impl PyGoal {
    /// Create an active, armed goal. Raises when the objective is blank.
    #[staticmethod]
    #[pyo3(signature = (objective, max_rounds = None))]
    fn create(objective: String, max_rounds: Option<u32>) -> PyResult<Self> {
        let goal = CoreGoal::new(objective, max_rounds).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err("a goal needs a non-blank objective")
        })?;
        Ok(Self { inner: goal })
    }

    /// Restore a goal from its JSON snapshot.
    #[staticmethod]
    fn from_json(json: String) -> PyResult<Self> {
        let mut goal: CoreGoal = serde_json::from_str(&json)
            .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
        goal.sanitize();
        Ok(Self { inner: goal })
    }

    /// The goal as a plain dictionary.
    fn to_json(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &goal_json(&self.inner))
    }

    #[getter]
    /// What the work is for, as one statement.
    fn objective(&self) -> String {
        self.inner.objective.clone()
    }

    #[getter]
    /// One of active, paused, completed or blocked.
    fn state(&self) -> String {
        state_name(self.inner.state).to_string()
    }

    #[getter]
    /// Rounds spent pursuing the goal.
    fn rounds(&self) -> u32 {
        self.inner.rounds
    }

    #[getter]
    /// The round cap.
    fn max_rounds(&self) -> u32 {
        self.inner.max_rounds
    }

    #[getter]
    /// armed or disarmed.
    fn activation(&self) -> String {
        self.inner.activation.as_str().to_string()
    }

    #[getter]
    /// The blocker, only while the goal is blocked.
    fn blocked_reason(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let value = self
            .inner
            .blocked_reason
            .as_ref()
            .map(|reason| {
                serde_json::json!({
                    "code": reason.code,
                    "message": reason.message,
                })
            })
            .unwrap_or(serde_json::Value::Null);
        json_to_py(py, &value)
    }

    #[getter]
    /// Rounds left before the cap.
    fn remaining(&self) -> u32 {
        self.inner.remaining()
    }

    #[getter]
    /// Whether another round may be counted right now.
    fn can_advance(&self) -> bool {
        self.inner.can_advance()
    }

    #[getter]
    /// Whether every round has been spent.
    fn exhausted(&self) -> bool {
        self.inner.exhausted()
    }

    #[getter]
    /// The largest objective this binding accepts, in characters.
    fn max_objective(&self) -> u32 {
        MAX_OBJECTIVE as u32
    }

    #[getter]
    /// The largest blocked explanation this binding accepts, in characters.
    fn max_reason(&self) -> u32 {
        MAX_REASON as u32
    }

    /// Count one round; False when the goal may not advance.
    fn advance(&mut self) -> bool {
        self.inner.advance()
    }

    /// Stop pursuing, keeping the option to resume.
    fn pause(&mut self) -> bool {
        self.inner.pause()
    }

    /// Take up a stopped goal again.
    fn resume(&mut self) -> bool {
        self.inner.resume()
    }

    /// Mark the objective achieved.
    fn complete(&mut self) -> bool {
        self.inner.complete()
    }

    /// Stop on a concrete condition; the code must be lower-kebab-case.
    fn block(&mut self, code: String, message: String) -> bool {
        self.inner.block(&code, message)
    }

    /// Allow automatic continuation without changing the phase.
    fn arm(&mut self) -> bool {
        self.inner.arm()
    }

    /// Drop continuation authority without changing the phase.
    fn disarm(&mut self) {
        self.inner.disarm();
    }

    /// Re-apply the field bounds and consistency rules in place.
    fn sanitize(&mut self) {
        self.inner.sanitize();
    }

    fn __repr__(&self) -> String {
        format!(
            "Goal(objective={:?}, state={:?}, rounds={}/{})",
            self.inner.objective,
            state_name(self.inner.state),
            self.inner.rounds,
            self.inner.max_rounds
        )
    }
}

/// The goal handle one running turn's tools write to.
///
/// A host seeds it with the session's current goal, hands it to
/// Agent.add_goal_tools, and applies take_settle when the turn ends.
#[pyclass(name = "GoalStore", skip_from_py_object)]
#[derive(Clone)]
pub struct PyGoalStore {
    pub(crate) inner: agent::tools::GoalStore,
}

#[pymethods]
impl PyGoalStore {
    #[new]
    /// Seed a store from goal.
    fn new(goal: &PyGoal) -> Self {
        Self {
            inner: agent::tools::GoalStore::new(goal.inner.clone()),
        }
    }

    /// The goal as the model sees it now.
    fn snapshot(&self, py: Python<'_>) -> PyResult<Option<Py<PyGoal>>> {
        match self.inner.snapshot() {
            Some(goal) => Ok(Some(Py::new(py, PyGoal { inner: goal })?)),
            None => Ok(None),
        }
    }

    /// Mark the objective achieved.
    fn complete(&self) -> bool {
        self.inner.complete()
    }

    /// Record a blocker; the code must be lower-kebab-case.
    fn block(&self, code: String, message: String) -> bool {
        self.inner.block(&code, &message)
    }

    /// Take the transition the model recorded, if any.
    fn take_settle(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let value = match self.inner.take_settle() {
            Some(agent::tools::GoalSettle::Complete) => {
                serde_json::json!({ "type": "complete" })
            }
            Some(agent::tools::GoalSettle::Blocked { code, message }) => serde_json::json!({
                "type": "blocked",
                "code": code,
                "message": message,
            }),
            None => serde_json::Value::Null,
        };
        json_to_py(py, &value)
    }

    /// Whether a settle transition is still waiting for the host.
    fn has_settle(&self) -> bool {
        self.inner.has_settle()
    }
}
