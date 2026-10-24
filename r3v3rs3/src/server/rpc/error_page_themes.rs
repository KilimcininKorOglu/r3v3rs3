use super::RpcMethod;
use crate::audit::AuditRecord;
use crate::server::state::ServerState;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::error_page_theme::ErrorPageTheme;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::ProxyKind;

pub struct GetErrorPageThemes;

#[async_trait::async_trait]
impl RpcMethod for GetErrorPageThemes {
    type Output = Vec<ErrorPageTheme>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let themes = if state.config().error_page_themes.is_empty() {
            r3v3rs3_api::error_page_theme::default_builtin_themes()
        } else {
            state.config().error_page_themes.clone()
        };
        Ok(themes)
    }
}

pub struct GetErrorPageTheme {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for GetErrorPageTheme {
    type Output = ErrorPageTheme;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let themes = if state.config().error_page_themes.is_empty() {
            r3v3rs3_api::error_page_theme::default_builtin_themes()
        } else {
            state.config().error_page_themes.clone()
        };
        themes
            .into_iter()
            .find(|theme| theme.id == self.id)
            .ok_or(Error::ErrorPageThemeNotFound { id: self.id })
    }
}

pub struct AddErrorPageTheme {
    pub theme: ErrorPageTheme,
}

#[async_trait::async_trait]
impl RpcMethod for AddErrorPageTheme {
    type Output = ShortId;
    const MUTATES: bool = true;

    async fn call(mut self, state: &mut ServerState) -> Result<Self::Output, Error> {
        self.theme.validate()?;
        let id = state.generate_id();
        self.theme.id = id;
        let mut config = state.config().clone();
        config.error_page_themes.push(self.theme.clone());
        state.set_config(config).await?;
        Ok(id)
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::AddErrorPageTheme).summary(self.theme.name.clone()))
    }
}

pub struct UpdateErrorPageTheme {
    pub theme: ErrorPageTheme,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateErrorPageTheme {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        self.theme.validate()?;
        let mut config = state.config().clone();
        let current = config
            .error_page_themes
            .iter_mut()
            .find(|t| t.id == self.theme.id)
            .ok_or(Error::ErrorPageThemeNotFound { id: self.theme.id })?;
        *current = self.theme.clone();
        state.set_config(config).await
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(
            AuditRecord::new(AuditAction::UpdateErrorPageTheme)
                .id(self.theme.id)
                .summary(self.theme.name.clone()),
        )
    }
}

pub struct DeleteErrorPageTheme {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for DeleteErrorPageTheme {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let in_use = state
            .proxies
            .entries()
            .any(|entry| match &entry.proxy.kind {
                ProxyKind::Http(h) => h.error_page_theme == Some(self.id),
                _ => false,
            });
        if in_use {
            return Err(Error::ErrorPageThemeInUse { id: self.id });
        }
        let mut config = state.config().clone();
        let prev_len = config.error_page_themes.len();
        config.error_page_themes.retain(|t| t.id != self.id);
        if config.error_page_themes.len() == prev_len {
            return Err(Error::ErrorPageThemeNotFound { id: self.id });
        }
        state.set_config(config).await
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::DeleteErrorPageTheme).id(self.id))
    }
}

pub struct ResetErrorPageThemes;

#[async_trait::async_trait]
impl RpcMethod for ResetErrorPageThemes {
    type Output = Vec<ErrorPageTheme>;
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let defaults = r3v3rs3_api::error_page_theme::default_builtin_themes();
        let mut config = state.config().clone();
        config.error_page_themes = defaults.clone();
        state.set_config(config).await?;
        Ok(defaults)
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(
            AuditRecord::new(AuditAction::UpdateConfig)
                .summary("Reset error page themes to defaults".to_string()),
        )
    }
}
