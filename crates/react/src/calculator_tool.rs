use crate::tool::{Tool, ToolError};
use serde_json::Value;

pub struct CalculatorTool;

impl Tool for CalculatorTool {
    fn name(&self) -> &str {
        "calculator"
    }
    fn description(&self) -> &str {
        "Evaluate simple arithmetic expressions"
    }
    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let expr = input
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if expr.is_empty() {
            return Err(ToolError::Failed("empty expression".to_string()));
        }
        match eval_expr(expr) {
            Ok(n) => Ok(Value::String(n.to_string())),
            Err(e) => Err(ToolError::Failed(e)),
        }
    }
}

fn eval_expr(expr: &str) -> Result<f64, String> {
    let s = expr.replace(" ", "");
    // naive single-operator arithmetic (supports +, -, *, / with two operands)
    if let Some(pos) = s.find('+') {
        let (a, b) = s.split_at(pos);
        let b = &b[1..];
        let a_val = a
            .parse::<f64>()
            .map_err(|_| "invalid left operand".to_string())?;
        let b_val = b
            .parse::<f64>()
            .map_err(|_| "invalid right operand".to_string())?;
        return Ok(a_val + b_val);
    }
    if let Some(pos) = s.find('*') {
        let (a, b) = s.split_at(pos);
        let b = &b[1..];
        let a_val = a
            .parse::<f64>()
            .map_err(|_| "invalid left operand".to_string())?;
        let b_val = b
            .parse::<f64>()
            .map_err(|_| "invalid right operand".to_string())?;
        return Ok(a_val * b_val);
    }
    if let Some(pos) = s.find('-') {
        if pos > 0 {
            let (a, b) = s.split_at(pos);
            let b = &b[1..];
            let a_val = a
                .parse::<f64>()
                .map_err(|_| "invalid left operand".to_string())?;
            let b_val = b
                .parse::<f64>()
                .map_err(|_| "invalid right operand".to_string())?;
            return Ok(a_val - b_val);
        }
    }
    if let Some(pos) = s.find('/') {
        let (a, b) = s.split_at(pos);
        let b = &b[1..];
        let a_val = a
            .parse::<f64>()
            .map_err(|_| "invalid left operand".to_string())?;
        let b_val = b
            .parse::<f64>()
            .map_err(|_| "invalid right operand".to_string())?;
        if b_val == 0.0 {
            return Err("division by zero".to_string());
        }
        return Ok(a_val / b_val);
    }
    // single number
    let v = s.parse::<f64>().map_err(|_| "invalid number".to_string())?;
    Ok(v)
}
