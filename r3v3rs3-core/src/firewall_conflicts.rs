//! Finds other firewalls of the host. nftables runs every input chain of every table, so a
//! packet that the r3v3rs3 table accepts is still dropped by a drop or reject of another table.

use crate::firewall::TABLE;
use serde_json::Value;
use std::collections::BTreeSet;
use tokio::process::Command;

/// The warnings about other firewalls: an active firewalld or ufw service, and every input
/// chain of another table that drops or rejects packets.
pub async fn detect() -> Vec<String> {
    let mut warnings = Vec::new();
    for service in ["firewalld", "ufw"] {
        if service_active(service).await {
            warnings.push(format!(
                "{service} is active: ports opened by r3v3rs3 can still be blocked"
            ));
        }
    }
    match ruleset().await {
        Ok(json) => warnings.extend(blocking_chains(&json).into_iter().map(|chain| {
            format!("the input chain {chain} drops or rejects packets: ports opened by r3v3rs3 can still be blocked")
        })),
        Err(err) => warnings.push(format!("cannot read the nftables ruleset: {err:#}")),
    }
    warnings
}

async fn service_active(name: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", name])
        .status()
        .await
        .is_ok_and(|status| status.success())
}

async fn ruleset() -> anyhow::Result<Value> {
    let output = Command::new("nft")
        .args(["-j", "list", "ruleset"])
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "nft failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// The `family table chain` names of the input-hook chains outside the r3v3rs3 table that have
/// a drop policy or a drop or reject rule, from the output of `nft -j list ruleset`.
pub fn blocking_chains(ruleset: &Value) -> BTreeSet<String> {
    let items = ruleset["nftables"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let inputs: BTreeSet<(String, String, String)> = items
        .iter()
        .filter_map(|item| item.get("chain"))
        .filter(|chain| chain["hook"] == "input" && !is_own(chain))
        .filter_map(chain_key)
        .collect();
    let dropping_policy = items
        .iter()
        .filter_map(|item| item.get("chain"))
        .filter(|chain| chain["policy"] == "drop")
        .filter_map(chain_key);
    let dropping_rules = items
        .iter()
        .filter_map(|item| item.get("rule"))
        .filter(|rule| drops(rule))
        .filter_map(|rule| {
            Some((
                rule["family"].as_str()?.to_string(),
                rule["table"].as_str()?.to_string(),
                rule["chain"].as_str()?.to_string(),
            ))
        });
    dropping_policy
        .chain(dropping_rules)
        .filter(|key| inputs.contains(key))
        .map(|(family, table, chain)| format!("{family} {table} {chain}"))
        .collect()
}

fn is_own(chain: &Value) -> bool {
    chain["family"] == "inet" && chain["table"] == TABLE
}

fn chain_key(chain: &Value) -> Option<(String, String, String)> {
    Some((
        chain["family"].as_str()?.to_string(),
        chain["table"].as_str()?.to_string(),
        chain["name"].as_str()?.to_string(),
    ))
}

fn drops(rule: &Value) -> bool {
    rule["expr"].as_array().is_some_and(|exprs| {
        exprs
            .iter()
            .any(|expr| expr.get("drop").is_some() || expr.get("reject").is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `nft -j list ruleset` output (nft 1.0.9) with the r3v3rs3 table, an iptables-nft
    /// INPUT chain that ends with a reject, a FORWARD chain with a drop policy, another input
    /// chain with a drop policy and a regular chain with a drop rule.
    const RULESET: &str = include_str!("testdata/nft-ruleset.json");

    #[test]
    fn only_dropping_input_chains_of_other_tables_are_reported() {
        let ruleset: Value = serde_json::from_str(RULESET).unwrap();
        let chains: Vec<String> = blocking_chains(&ruleset).into_iter().collect();
        // The own table drops by design; the forward hook is Docker's path; a chain without a
        // hook runs only when an input chain jumps to it.
        assert_eq!(chains, ["inet other inp", "ip filter INPUT"]);
    }

    #[test]
    fn an_empty_or_strange_ruleset_gives_no_warning() {
        for text in [
            r#"{"nftables": []}"#,
            r#"{}"#,
            r#"{"nftables": [{"chain": 5}]}"#,
        ] {
            let ruleset: Value = serde_json::from_str(text).unwrap();
            assert!(blocking_chains(&ruleset).is_empty(), "{text}");
        }
    }
}
