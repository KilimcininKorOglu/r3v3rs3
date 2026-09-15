//! An IP filter and an authentication policy that several proxies and routes share.

use crate::id::ShortId;
use crate::policy::{AuthPolicy, IpFilter};
use crate::proxy::HttpProxy;
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
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

impl AccessList {
    /// A list that rejects every client. A proxy or a route whose access list does not exist uses
    /// it.
    pub fn deny_all() -> Self {
        Self {
            name: String::new(),
            ip_filter: IpFilter {
                allow: Vec::new(),
                deny: vec![IpNet::V4(Ipv4Net::default()), IpNet::V6(Ipv6Net::default())],
            },
            auth: AuthPolicy::None,
        }
    }
}

/// Replaces the IP filter and the authentication of an HTTP proxy and its routes with those of
/// their access lists. The list of a route replaces the settings of the route, and the list of the
/// proxy replaces the settings of the proxy. Returns the ids that no list has.
pub fn apply_access_lists(http: &mut HttpProxy, lists: &[AccessListEntry]) -> Vec<ShortId> {
    let mut missing = Vec::new();
    let mut find = |id: ShortId| match lists.iter().find(|entry| entry.id == id) {
        Some(entry) => entry.list.clone(),
        None => {
            missing.push(id);
            AccessList::deny_all()
        }
    };
    if let Some(id) = http.access_list {
        let list = find(id);
        http.ip_filter = list.ip_filter;
        http.auth = list.auth;
    }
    for route in &mut http.routes {
        if let Some(id) = route.access_list {
            let list = find(id);
            route.ip_filter = Some(list.ip_filter);
            route.auth = Some(list.auth);
        }
    }
    missing
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

    #[test]
    fn the_list_of_a_route_or_a_proxy_replaces_its_settings() {
        let id = |value: &str| value.parse::<ShortId>().unwrap();
        let office = AccessList {
            name: "Office".into(),
            ip_filter: IpFilter {
                allow: parse_cidr_list("10.0.0.0/8").unwrap(),
                deny: Vec::new(),
            },
            auth: AuthPolicy::Session,
        };
        let lists = [AccessListEntry {
            id: id("office"),
            list: office.clone(),
        }];
        let route = |value: serde_json::Value| serde_json::from_value(value).unwrap();
        let mut http = HttpProxy {
            access_list: Some(id("office")),
            routes: vec![
                route(json!({"path": "/"})),
                route(json!({"path": "/open", "ip_filter": {}, "auth": {"type": "none"}})),
                route(json!({"path": "/gone", "access_list": "gone"})),
            ],
            ..Default::default()
        };

        assert_eq!(apply_access_lists(&mut http, &lists), vec![id("gone")]);
        assert_eq!(
            (&http.ip_filter, &http.auth),
            (&office.ip_filter, &office.auth)
        );
        assert_eq!(
            (&http.routes[0].ip_filter, &http.routes[0].auth),
            (&None, &None)
        );
        assert_eq!(http.routes[1].ip_filter, Some(IpFilter::default()));
        assert_eq!(http.routes[1].auth, Some(AuthPolicy::None));

        let denied = http.routes[2].ip_filter.clone().unwrap();
        for client in ["192.0.2.1", "2001:db8::1", "::ffff:10.0.0.1"] {
            assert!(!denied.allows(client.parse().unwrap()), "{client}");
        }
    }
}
