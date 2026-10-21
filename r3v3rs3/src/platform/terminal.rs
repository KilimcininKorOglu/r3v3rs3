//! Command resolution for container terminal sessions and database CLI shortcuts.

/// Resolves the command line arguments to execute for an interactive container terminal session.
pub fn resolve_shell_command(shell: Option<&str>, db_engine: Option<&str>) -> Vec<String> {
    let clean = shell.map(str::trim).filter(|s| !s.is_empty());
    match clean {
        None | Some("sh") | Some("/bin/sh") => vec!["/bin/sh".into()],
        Some("bash") | Some("/bin/bash") => bash_command(),
        Some("zsh") | Some("/bin/zsh") => zsh_command(),
        Some("cli") | Some("db") => match db_engine {
            Some(engine) => db_cli_command(engine),
            None => vec!["/bin/sh".into()],
        },
        Some(custom) => resolve_custom_shell(custom),
    }
}

/// Fallback wrapper for bash.
fn bash_command() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "exec bash 2>/dev/null || exec /bin/bash 2>/dev/null || { echo 'bash is not installed, falling back to /bin/sh...'; exec /bin/sh; }".into(),
    ]
}

/// Fallback wrapper for zsh.
fn zsh_command() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "exec zsh 2>/dev/null || exec /bin/zsh 2>/dev/null || { echo 'zsh is not installed, falling back to /bin/sh...'; exec /bin/sh; }".into(),
    ]
}

/// Command resolution for custom shell input or DB CLI tool names.
fn resolve_custom_shell(custom: &str) -> Vec<String> {
    if let Some(cmd) = db_cli_shortcut(custom) {
        cmd
    } else if has_shell_metachars(custom) {
        vec!["sh".into(), "-c".into(), format!("exec {custom}")]
    } else {
        vec![custom.into()]
    }
}

fn has_shell_metachars(cmd: &str) -> bool {
    cmd.contains(' ') || cmd.contains('&') || cmd.contains('|') || cmd.contains(';')
}

/// Command resolution for specific database CLI tool names.
fn db_cli_shortcut(tool: &str) -> Option<Vec<String>> {
    match tool {
        "psql" => Some(postgres_cli()),
        "mysql" => Some(mysql_cli()),
        "mariadb" => Some(mariadb_cli()),
        "redis" | "redis-cli" => Some(redis_cli()),
        "mongo" | "mongosh" => Some(mongo_cli()),
        _ => None,
    }
}

/// Command for a database engine.
fn db_cli_command(engine: &str) -> Vec<String> {
    match engine {
        "postgres" => postgres_cli(),
        "mysql" => mysql_cli(),
        "mariadb" => mariadb_cli(),
        "redis" => redis_cli(),
        "mongodb" => mongo_cli(),
        _ => vec!["/bin/sh".into()],
    }
}

fn postgres_cli() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "if [ -n \"$POSTGRES_PASSWORD\" ]; then PGPASSWORD=\"$POSTGRES_PASSWORD\" exec psql -U \"${POSTGRES_USER:-postgres}\" -d \"${POSTGRES_DB:-postgres}\" 2>/dev/null || exec psql; else exec psql -U \"${POSTGRES_USER:-postgres}\" 2>/dev/null || exec psql; fi || { echo 'psql failed, falling back to /bin/sh...'; exec /bin/sh; }".into(),
    ]
}

fn mysql_cli() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "exec mysql -u root -p\"$MYSQL_ROOT_PASSWORD\" 2>/dev/null || exec mysql -u root 2>/dev/null || exec mariadb -u root 2>/dev/null || { echo 'mysql client failed, falling back to /bin/sh...'; exec /bin/sh; }".into(),
    ]
}

fn mariadb_cli() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "exec mariadb -u root -p\"$MARIADB_ROOT_PASSWORD\" 2>/dev/null || exec mariadb -u root 2>/dev/null || exec mysql -u root 2>/dev/null || { echo 'mariadb client failed, falling back to /bin/sh...'; exec /bin/sh; }".into(),
    ]
}

fn redis_cli() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "if [ -n \"$REDIS_PASSWORD\" ]; then exec redis-cli -a \"$REDIS_PASSWORD\" 2>/dev/null || exec redis-cli; else exec redis-cli; fi || { echo 'redis-cli failed, falling back to /bin/sh...'; exec /bin/sh; }".into(),
    ]
}

fn mongo_cli() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "if [ -n \"$MONGO_INITDB_ROOT_PASSWORD\" ]; then exec mongosh -u root -p \"$MONGO_INITDB_ROOT_PASSWORD\" --authenticationDatabase admin 2>/dev/null || exec mongosh 2>/dev/null || exec mongo; else exec mongosh 2>/dev/null || exec mongo; fi || { echo 'mongosh failed, falling back to /bin/sh...'; exec /bin/sh; }".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_is_sh() {
        assert_eq!(resolve_shell_command(None, None), vec!["/bin/sh"]);
        assert_eq!(resolve_shell_command(Some(""), None), vec!["/bin/sh"]);
        assert_eq!(resolve_shell_command(Some("sh"), None), vec!["/bin/sh"]);
        assert_eq!(
            resolve_shell_command(Some("/bin/sh"), None),
            vec!["/bin/sh"]
        );
    }

    #[test]
    fn bash_and_zsh_commands() {
        let bash = resolve_shell_command(Some("bash"), None);
        assert_eq!(bash[0], "sh");
        assert_eq!(bash[1], "-c");
        assert!(bash[2].contains("bash"));

        let zsh = resolve_shell_command(Some("zsh"), None);
        assert_eq!(zsh[0], "sh");
        assert_eq!(zsh[1], "-c");
        assert!(zsh[2].contains("zsh"));
    }

    #[test]
    fn db_cli_by_engine_and_shortcut() {
        let psql = resolve_shell_command(Some("cli"), Some("postgres"));
        assert!(psql[2].contains("psql"));

        let psql_direct = resolve_shell_command(Some("psql"), None);
        assert_eq!(psql, psql_direct);

        let redis = resolve_shell_command(Some("redis-cli"), None);
        assert!(redis[2].contains("redis-cli"));

        let mysql = resolve_shell_command(Some("mysql"), None);
        assert!(mysql[2].contains("mysql"));

        let mariadb = resolve_shell_command(Some("mariadb"), None);
        assert!(mariadb[2].contains("mariadb"));

        let mongo = resolve_shell_command(Some("mongosh"), None);
        assert!(mongo[2].contains("mongosh"));

        let cli_fallback = resolve_shell_command(Some("cli"), None);
        assert_eq!(cli_fallback, vec!["/bin/sh"]);
    }

    #[test]
    fn custom_commands() {
        let custom = resolve_shell_command(Some("/usr/bin/fish"), None);
        assert_eq!(custom, vec!["/usr/bin/fish"]);

        let custom_with_args = resolve_shell_command(Some("python3 -m http.server"), None);
        assert_eq!(custom_with_args[0], "sh");
        assert_eq!(custom_with_args[1], "-c");
        assert_eq!(custom_with_args[2], "exec python3 -m http.server");
    }
}
