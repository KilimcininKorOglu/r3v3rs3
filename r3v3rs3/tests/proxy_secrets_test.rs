use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use r3v3rs3::{
    config::storage::Storage,
    server::rpc::proxies::{GetProxy, GetProxyList, UpdateProxy},
};
use r3v3rs3_api::{
    error::Error,
    event::ServerEvent,
    policy::{AuthPolicy, BasicAuth, BasicAuthUser, BearerAuth, BearerToken},
    proxy::{HttpProxy, Proxy, ProxyEntry, ProxyKind},
};
use sha2::{Digest, Sha256};
use std::time::Duration;

mod common;
use common::{TestStorage, call, http_proxy_entry, http_route, with_server};

fn http(proxy: &mut Proxy) -> &mut HttpProxy {
    match &mut proxy.kind {
        ProxyKind::Http(http) => http,
        _ => panic!("expected an HTTP proxy"),
    }
}

fn basic_users(proxy: &mut Proxy) -> &mut Vec<BasicAuthUser> {
    match &mut http(proxy).auth {
        AuthPolicy::Basic(basic) => &mut basic.users,
        _ => panic!("expected basic auth"),
    }
}

fn route_token(proxy: &mut Proxy) -> &mut BearerToken {
    match http(proxy).routes[0].auth.as_mut() {
        Some(AuthPolicy::Bearer(bearer)) => &mut bearer.tokens[0],
        _ => panic!("expected bearer auth on the route"),
    }
}

/// A proxy with a Basic user on the proxy and a bearer token on its route.
fn stored_entry() -> ProxyEntry {
    let salt = SaltString::generate(OsRng);
    let password_hash = Argon2::default()
        .hash_password(b"alice-secret", &salt)
        .unwrap()
        .to_string();
    let mut route = http_route("/", "http://127.0.0.1:1/", None);
    route.auth = Some(AuthPolicy::Bearer(BearerAuth {
        tokens: vec![BearerToken {
            name: "ci".into(),
            token_hash: hex::encode(Sha256::digest("token-for-ci-0123456789")),
            ..Default::default()
        }],
    }));
    let proxy = HttpProxy {
        routes: vec![route],
        auth: AuthPolicy::Basic(BasicAuth {
            realm: String::new(),
            users: vec![BasicAuthUser {
                username: "alice".into(),
                password_hash,
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    http_proxy_entry("proxy1", "port", proxy)
}

#[tokio::test]
async fn the_admin_api_hides_the_auth_hashes_and_an_update_keeps_them() -> anyhow::Result<()> {
    let stored = stored_entry();
    let storage = TestStorage::builder().proxies(vec![stored.clone()]).build();
    with_server(storage.clone(), |mut channels| async move {
        let mut original = stored;
        let id = original.id;
        let mut entry = call(&mut channels, GetProxy { id }).await??;
        assert_eq!(
            call(&mut channels, GetProxyList).await??,
            vec![entry.clone()]
        );
        assert!(basic_users(&mut entry.proxy)[0].password_hash.is_empty());
        assert!(basic_users(&mut entry.proxy)[0].password_set);
        assert!(route_token(&mut entry.proxy).token_hash.is_empty());
        assert!(route_token(&mut entry.proxy).token_set);

        let mut events = channels.event.subscribe();
        http(&mut entry.proxy).vhosts = vec!["example.com".parse()?];
        let update = UpdateProxy {
            entry: entry.clone(),
        };
        call(&mut channels, update).await??;
        let saved = storage.load_proxies().await;
        let mut saved = saved
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no proxy"))?;
        assert_eq!(
            basic_users(&mut saved.proxy)[0].password_hash,
            basic_users(&mut original.proxy)[0].password_hash
        );
        assert_eq!(
            route_token(&mut saved.proxy).token_hash,
            route_token(&mut original.proxy).token_hash
        );
        assert!(!basic_users(&mut saved.proxy)[0].password_set);

        let published = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ServerEvent::ProxiesUpdated { entries } = events.recv().await? {
                    return anyhow::Ok(entries);
                }
            }
        })
        .await??;
        assert_eq!(published, vec![entry.clone()]);

        basic_users(&mut entry.proxy).push(BasicAuthUser {
            username: "bob".into(),
            password_set: true,
            ..Default::default()
        });
        let result = call(&mut channels, UpdateProxy { entry }).await?;
        assert!(
            matches!(result, Err(Error::PasswordRequired { .. })),
            "{result:?}"
        );
        Ok(())
    })
    .await
}
