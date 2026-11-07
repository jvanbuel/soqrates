//! Reads the pull request number from a GitHub Actions event payload.

use serde_json::Value;

/// The PR number from a `pull_request` or `issue_comment` event payload.
/// `None` for an issue comment that is not on a pull request.
pub fn pr_number(event: &Value) -> Option<u64> {
    if let Some(n) = event
        .pointer("/pull_request/number")
        .and_then(Value::as_u64)
    {
        return Some(n);
    }
    let issue = event.get("issue")?;
    issue.get("pull_request")?;
    issue.get("number")?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::pr_number;
    use serde_json::json;

    #[test]
    fn numbers() {
        assert_eq!(pr_number(&json!({"pull_request": {"number": 7}})), Some(7));
        assert_eq!(
            pr_number(&json!({"issue": {"number": 8, "pull_request": {}}})),
            Some(8)
        );
        assert_eq!(pr_number(&json!({"issue": {"number": 9}})), None);
    }
}
