//! The environment variables that a new database server starts with.

use r3v3rs3_api::error::Error;
use r3v3rs3_api::platform::{EngineInfo, EnvEntry};

/// Builds the initial environment entries for provisioning a database container.
pub fn initial_db_env_entries(info: &EngineInfo, password: &str) -> Result<Vec<EnvEntry>, Error> {
    if info.password_env.is_empty() {
        return Ok(Vec::new());
    }
    let mut entries = vec![EnvEntry {
        key: info.password_env.parse()?,
        value: Some(password.to_string()),
        secret: true,
    }];
    if info.engine == "mongodb" && !info.user.is_empty() {
        entries.push(EnvEntry {
            key: "MONGO_INITDB_ROOT_USERNAME".parse()?,
            value: Some(info.user.clone()),
            secret: false,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mongodb_initial_env_entries() {
        let info = EngineInfo {
            engine: "mongodb".to_string(),
            label: "MongoDB".to_string(),
            versions: vec!["mongo:8".to_string()],
            container_port: 27017,
            password_env: "MONGO_INITDB_ROOT_PASSWORD".to_string(),
            user: "root".to_string(),
            default_db: "admin".to_string(),
            url_scheme: "mongodb".to_string(),
            env_prefix: "MONGO".to_string(),
            url_database: true,
            data_path: "/data/db".to_string(),
            version_data_paths: Default::default(),
        };
        let entries = initial_db_env_entries(&info, "secret123").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key.as_str(), "MONGO_INITDB_ROOT_PASSWORD");
        assert_eq!(entries[0].value.as_deref(), Some("secret123"));
        assert!(entries[0].secret);
        assert_eq!(entries[1].key.as_str(), "MONGO_INITDB_ROOT_USERNAME");
        assert_eq!(entries[1].value.as_deref(), Some("root"));
        assert!(!entries[1].secret);
    }

    #[test]
    fn test_postgres_initial_env_entries() {
        let info = EngineInfo {
            engine: "postgres".to_string(),
            label: "PostgreSQL".to_string(),
            versions: vec!["postgres:17".to_string()],
            container_port: 5432,
            password_env: "POSTGRES_PASSWORD".to_string(),
            user: "postgres".to_string(),
            default_db: "postgres".to_string(),
            url_scheme: "postgres".to_string(),
            env_prefix: "POSTGRES".to_string(),
            url_database: true,
            data_path: "/var/lib/postgresql/data".to_string(),
            version_data_paths: Default::default(),
        };
        let entries = initial_db_env_entries(&info, "pass456").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key.as_str(), "POSTGRES_PASSWORD");
        assert_eq!(entries[0].value.as_deref(), Some("pass456"));
        assert!(entries[0].secret);
    }

    #[test]
    fn test_redis_initial_env_entries() {
        let info = EngineInfo {
            engine: "redis".to_string(),
            label: "Redis".to_string(),
            versions: vec!["redis:8-alpine".to_string()],
            container_port: 6379,
            password_env: String::new(),
            user: String::new(),
            default_db: String::new(),
            url_scheme: "redis".to_string(),
            env_prefix: "REDIS".to_string(),
            url_database: false,
            data_path: "/data".to_string(),
            version_data_paths: Default::default(),
        };
        let entries = initial_db_env_entries(&info, "").unwrap();
        assert!(entries.is_empty());
    }
}
