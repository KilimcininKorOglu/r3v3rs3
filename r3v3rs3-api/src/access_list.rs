//! An IP filter and an authentication policy that several proxies and routes share.

use crate::id::ShortId;
use crate::policy::{AuthPolicy, IpFilter};
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AccessList {
    #[schema(example = "Office")]
    pub name: String,
    #[serde(default, skip_serializing_if = "IpFilter::is_empty")]
    pub ip_filter: IpFilter,
    #[serde(default, skip_serializing_if = "AuthPolicy::is_none")]
    pub auth: AuthPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AccessListEntry {
    pub id: ShortId,
    #[schema(inline)]
    #[serde(flatten)]
    pub list: AccessList,
}

impl From<(ShortId, AccessList)> for AccessListEntry {
    fn from((id, list): (ShortId, AccessList)) -> Self {
        Self { id, list }
    }
}

impl From<AccessListEntry> for (ShortId, AccessList) {
    fn from(entry: AccessListEntry) -> Self {
        (entry.id, entry.list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cidr::parse_cidr_list;
    use crate::policy::BearerAuth;
    use serde_json::json;

    #[test]
    fn an_entry_holds_the_list_fields_next_to_its_id() {
        let entry = AccessListEntry {
            id: "office".parse().unwrap(),
            list: AccessList {
                name: "Office".into(),
                ip_filter: IpFilter {
                    allow: parse_cidr_list("10.0.0.0/8").unwrap(),
                    deny: Vec::new(),
                },
                auth: AuthPolicy::Bearer(BearerAuth::default()),
            },
        };
        let value = json!({
            "id": "office",
            "name": "Office",
            "ip_filter": { "allow": ["10.0.0.0/8"] },
            "auth": { "type": "bearer", "tokens": [] },
        });
        assert_eq!(serde_json::to_value(&entry).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<AccessListEntry>(value).unwrap(),
            entry
        );

        let open = serde_json::from_value::<AccessList>(json!({ "name": "Open" })).unwrap();
        assert!(open.ip_filter.is_empty() && open.auth.is_none());
    }
}
