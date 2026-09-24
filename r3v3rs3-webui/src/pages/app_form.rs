//! The fields of the app form and the request that they make.

use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::resource_page::text_input;
use r3v3rs3_api::container::{ResourceLimits, RestartPolicy, VolumeMount};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git::{GitRef, RelPath};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, AppRequest, AppSource, AppSpec, LOCAL_TARGET, TargetEntry};
use std::str::FromStr;
use web_sys::{HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;

const BYTES_PER_MB: u64 = 1024 * 1024;
const NANO_CPUS_PER_CPU: f64 = 1_000_000_000.0;

/// Each source kind with its value in the select and the translation key of its name.
const KINDS: [(SourceKind, &str, &str); 3] = [
    (SourceKind::Image, "image", "apps.source_image"),
    (SourceKind::Git, "git", "apps.source_git"),
    (SourceKind::Compose, "compose", "apps.source_compose"),
];

/// Each restart policy with its value in the select and the translation key of its name.
const RESTARTS: [(RestartPolicy, &str, &str); 4] = [
    (
        RestartPolicy::UnlessStopped,
        "unless-stopped",
        "apps.restart_unless_stopped",
    ),
    (RestartPolicy::Always, "always", "apps.restart_always"),
    (
        RestartPolicy::OnFailure,
        "on-failure",
        "apps.restart_on_failure",
    ),
    (RestartPolicy::No, "no", "apps.restart_no"),
];

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum SourceKind {
    #[default]
    Image,
    Git,
    Compose,
}

/// The raw values of the form. Every source kind keeps its own fields, so a change of the kind
/// loses no input.
#[derive(Clone, PartialEq, Debug)]
pub struct AppForm {
    pub name: String,
    pub target: String,
    pub kind: SourceKind,
    pub image: String,
    pub repository: String,
    pub branch: String,
    pub context: String,
    pub dockerfile: String,
    pub compose_file: String,
    pub service: String,
    pub port: String,
    /// One domain on each line.
    pub domains: String,
    pub health_check_path: String,
    /// One mount on each line: `volume:/path` or `volume:/path:ro`.
    pub volumes: String,
    pub restart: RestartPolicy,
    pub memory_mb: String,
    pub cpus: String,
}

impl Default for AppForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            target: LOCAL_TARGET.to_string(),
            kind: SourceKind::default(),
            image: String::new(),
            repository: String::new(),
            branch: GitRef::main().to_string(),
            context: RelPath::root().to_string(),
            dockerfile: RelPath::dockerfile().to_string(),
            compose_file: String::new(),
            service: String::new(),
            port: String::new(),
            domains: String::new(),
            health_check_path: String::new(),
            volumes: String::new(),
            restart: RestartPolicy::default(),
            memory_mb: String::new(),
            cpus: String::new(),
        }
    }
}

impl AppForm {
    pub fn edit(app: &AppEntry) -> Self {
        let spec = &app.spec;
        let form = Self {
            name: app.name.to_string(),
            target: app.target.to_string(),
            port: spec.port.to_string(),
            domains: lines(spec.domains.iter()),
            health_check_path: spec.health_check_path.clone().unwrap_or_default(),
            volumes: lines(spec.volumes.iter().map(mount_line)),
            restart: spec.restart,
            memory_mb: optional(spec.limits.memory_bytes.map(|bytes| bytes / BYTES_PER_MB)),
            cpus: optional(
                spec.limits
                    .nano_cpus
                    .map(|nano| nano as f64 / NANO_CPUS_PER_CPU),
            ),
            ..Self::default()
        };
        form.with_source(&spec.source)
    }

    fn with_source(self, source: &AppSource) -> Self {
        match source {
            AppSource::Image { image } => Self {
                kind: SourceKind::Image,
                image: image.to_string(),
                ..self
            },
            AppSource::Git {
                repository,
                branch,
                context,
                dockerfile,
            } => Self {
                kind: SourceKind::Git,
                repository: repository.to_string(),
                branch: branch.to_string(),
                context: context.to_string(),
                dockerfile: dockerfile.to_string(),
                ..self
            },
            AppSource::Compose {
                repository,
                branch,
                file,
                service,
            } => Self {
                kind: SourceKind::Compose,
                repository: repository.to_string(),
                branch: branch.to_string(),
                compose_file: optional(file.as_ref()),
                service: service.to_string(),
                ..self
            },
        }
    }

    /// Whether the source has a Git repository, so the app can use a Git token.
    pub fn uses_git(&self) -> bool {
        self.kind != SourceKind::Image
    }

    /// Checks the form and returns the request. The error is a message in the selected language.
    pub fn parse(&self, locale: Locale) -> Result<AppRequest, String> {
        let (volumes, restart, limits) = self.container_settings(locale)?;
        let spec = AppSpec {
            source: self.source(locale)?,
            port: self.port(locale)?,
            domains: parse_lines(locale, &self.domains, parse)?,
            health_check_path: non_empty(&self.health_check_path).map(str::to_string),
            volumes,
            restart,
            limits,
        };
        spec.validate().map_err(|err| locale.error_message(&err))?;
        Ok(AppRequest {
            name: parse(locale, self.name.trim())?,
            target: parse_target(locale, &self.target)?,
            spec,
        })
    }

    fn source(&self, locale: Locale) -> Result<AppSource, String> {
        match self.kind {
            SourceKind::Image => Ok(AppSource::Image {
                image: parse(locale, self.image.trim())?,
            }),
            SourceKind::Git => Ok(AppSource::Git {
                repository: parse(locale, self.repository.trim())?,
                branch: parse(locale, self.branch.trim())?,
                context: parse(locale, self.context.trim())?,
                dockerfile: parse(locale, self.dockerfile.trim())?,
            }),
            SourceKind::Compose => self.compose_source(locale),
        }
    }

    fn compose_source(&self, locale: Locale) -> Result<AppSource, String> {
        let file = non_empty(&self.compose_file)
            .map(|file| parse(locale, file))
            .transpose()?;
        Ok(AppSource::Compose {
            repository: parse(locale, self.repository.trim())?,
            branch: parse(locale, self.branch.trim())?,
            file,
            service: parse(locale, self.service.trim())?,
        })
    }

    fn port(&self, locale: Locale) -> Result<u16, String> {
        match self.port.trim().parse::<u16>() {
            Ok(port) if port > 0 => Ok(port),
            _ => Err(locale.t("apps.invalid_port").to_string()),
        }
    }

    /// The volumes, the restart policy and the limits. A Compose file sets them itself, so a
    /// Compose app sends the defaults.
    fn container_settings(
        &self,
        locale: Locale,
    ) -> Result<(Vec<VolumeMount>, RestartPolicy, ResourceLimits), String> {
        if self.kind == SourceKind::Compose {
            return Ok(Default::default());
        }
        let volumes = parse_lines(locale, &self.volumes, parse_mount)?;
        Ok((volumes, self.restart, self.limits(locale)?))
    }

    fn limits(&self, locale: Locale) -> Result<ResourceLimits, String> {
        let memory_bytes = non_empty(&self.memory_mb)
            .map(|value| parse_memory(locale, value))
            .transpose()?;
        let nano_cpus = non_empty(&self.cpus)
            .map(|value| parse_cpus(locale, value))
            .transpose()?;
        Ok(ResourceLimits {
            memory_bytes,
            nano_cpus,
        })
    }
}

fn parse<T: FromStr<Err = Error>>(locale: Locale, value: &str) -> Result<T, String> {
    value.parse().map_err(|err| locale.error_message(&err))
}

fn parse_target(locale: Locale, value: &str) -> Result<ShortId, String> {
    // An empty value parses as the zero id, so it is an error here.
    match value.parse::<ShortId>() {
        Ok(id) if !value.is_empty() => Ok(id),
        _ => Err(locale.t("apps.invalid_target").to_string()),
    }
}

fn parse_mount(locale: Locale, line: &str) -> Result<VolumeMount, String> {
    let invalid = || locale.tf("apps.invalid_volume", &[("volume", line)]);
    let mut parts = line.split(':');
    let (Some(volume), Some(target)) = (parts.next(), parts.next()) else {
        return Err(invalid());
    };
    let read_only = match parts.next() {
        None => false,
        Some("ro") => true,
        Some(_) => return Err(invalid()),
    };
    if parts.next().is_some() {
        return Err(invalid());
    }
    Ok(VolumeMount {
        volume: parse(locale, volume)?,
        target: parse(locale, target)?,
        read_only,
    })
}

fn parse_memory(locale: Locale, value: &str) -> Result<u64, String> {
    match value.parse::<u64>() {
        Ok(mb) if mb > 0 => mb
            .checked_mul(BYTES_PER_MB)
            .ok_or_else(|| locale.t("apps.invalid_memory").to_string()),
        _ => Err(locale.t("apps.invalid_memory").to_string()),
    }
}

fn parse_cpus(locale: Locale, value: &str) -> Result<u64, String> {
    match value.parse::<f64>() {
        Ok(cpus) if cpus.is_finite() && cpus > 0.0 && cpus <= 1024.0 => {
            Ok((cpus * NANO_CPUS_PER_CPU).round() as u64)
        }
        _ => Err(locale.t("apps.invalid_cpus").to_string()),
    }
}

/// Parses every line that is not blank.
fn parse_lines<T>(
    locale: Locale,
    text: &str,
    parse_line: fn(Locale, &str) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| parse_line(locale, line))
        .collect()
}

fn non_empty(value: &str) -> Option<&str> {
    Some(value.trim()).filter(|value| !value.is_empty())
}

fn optional(value: Option<impl ToString>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn lines<T: ToString>(items: impl Iterator<Item = T>) -> String {
    items
        .map(|item| item.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn mount_line(mount: &VolumeMount) -> String {
    let suffix = if mount.read_only { ":ro" } else { "" };
    format!("{}:{}{suffix}", mount.volume, mount.target.as_str())
}

/// The fields of the form. `editing` locks the name of an existing app.
pub fn fields_view(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    targets: &[TargetEntry],
    editing: bool,
) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("common.name")}</label>
            <input type="text" autocapitalize="off" autocomplete="off" value={form.name.clone()} oninput={text_input(form, |f| &mut f.name)} disabled={editing} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t("apps.name_hint")}</p>
            { target_select(locale, form, targets) }
            { kind_select(locale, form) }
            { source_view(locale, form) }
            { text_field(locale, form, "apps.port", Some("apps.port_hint"), |f| &mut f.port) }
            { text_area(locale, form, "apps.domains", "apps.domains_hint", |f| &mut f.domains) }
            { text_field(locale, form, "apps.health_check_path", Some("apps.health_check_path_hint"), |f| &mut f.health_check_path) }
            if form.kind != SourceKind::Compose {
                { container_view(locale, form) }
            }
        </>
    }
}

fn source_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    match form.kind {
        SourceKind::Image => text_field(locale, form, "apps.image", Some("apps.image_hint"), |f| {
            &mut f.image
        }),
        SourceKind::Git => html! {
            <>
                { repository_view(locale, form) }
                { text_field(locale, form, "apps.context", Some("apps.context_hint"), |f| &mut f.context) }
                { text_field(locale, form, "apps.dockerfile", Some("apps.dockerfile_hint"), |f| &mut f.dockerfile) }
            </>
        },
        SourceKind::Compose => html! {
            <>
                { repository_view(locale, form) }
                { text_field(locale, form, "apps.compose_file", Some("apps.compose_file_hint"), |f| &mut f.compose_file) }
                { text_field(locale, form, "apps.service", Some("apps.service_hint"), |f| &mut f.service) }
            </>
        },
    }
}

fn repository_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    html! {
        <>
            { text_field(locale, form, "apps.repository", Some("apps.repository_hint"), |f| &mut f.repository) }
            { text_field(locale, form, "apps.branch", None, |f| &mut f.branch) }
        </>
    }
}

/// The volumes, the restart policy and the limits. A Compose app sets them in its Compose file.
fn container_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    html! {
        <>
            { text_area(locale, form, "apps.volumes", "apps.volumes_hint", |f| &mut f.volumes) }
            { restart_select(locale, form) }
            { text_field(locale, form, "apps.memory", Some("apps.memory_hint"), |f| &mut f.memory_mb) }
            { text_field(locale, form, "apps.cpus", Some("apps.cpus_hint"), |f| &mut f.cpus) }
        </>
    }
}

fn text_field(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    label: &'static str,
    hint: Option<&'static str>,
    select: fn(&mut AppForm) -> &mut String,
) -> Html {
    let mut current = (**form).clone();
    let value = select(&mut current).clone();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t(label)}</label>
            <input type="text" autocapitalize="off" autocomplete="off" {value} oninput={text_input(form, select)} class={INPUT_CLASS} />
            if let Some(hint) = hint {
                <p class={HINT_CLASS}>{locale.t(hint)}</p>
            }
        </>
    }
}

fn text_area(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    label: &'static str,
    hint: &'static str,
    select: fn(&mut AppForm) -> &mut String,
) -> Html {
    let mut current = (**form).clone();
    let value = select(&mut current).clone();
    let state = form.clone();
    let oninput = Callback::from(move |event: InputEvent| {
        let area: HtmlTextAreaElement = event.target_unchecked_into();
        let mut updated = (*state).clone();
        *select(&mut updated) = area.value();
        state.set(updated);
    });
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t(label)}</label>
            <textarea rows="3" autocapitalize="off" {value} {oninput} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t(hint)}</p>
        </>
    }
}

/// A select whose change writes the chosen option into the form.
fn select_view(
    form: &UseStateHandle<AppForm>,
    options: Html,
    apply: fn(&mut AppForm, &str),
) -> Html {
    let state = form.clone();
    let onchange = Callback::from(move |event: Event| {
        let select: HtmlSelectElement = event.target_unchecked_into();
        let mut updated = (*state).clone();
        apply(&mut updated, &select.value());
        state.set(updated);
    });
    html! { <select {onchange} class={INPUT_CLASS}>{options}</select> }
}

fn kind_select(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let options = KINDS
        .iter()
        .map(|(kind, value, key)| {
            html! { <option value={*value} selected={*kind == form.kind}>{locale.t(key)}</option> }
        })
        .collect::<Html>();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.source")}</label>
            { select_view(form, options, |f, value| f.kind = parse_kind(value)) }
        </>
    }
}

fn restart_select(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let options = RESTARTS
        .iter()
        .map(|(restart, value, key)| {
            html! { <option value={*value} selected={*restart == form.restart}>{locale.t(key)}</option> }
        })
        .collect::<Html>();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.restart")}</label>
            { select_view(form, options, |f, value| f.restart = parse_restart(value)) }
        </>
    }
}

fn target_select(locale: Locale, form: &UseStateHandle<AppForm>, targets: &[TargetEntry]) -> Html {
    let options = targets
        .iter()
        .map(|target| {
            let id = target.id.to_string();
            let (status, _) = super::targets::target_status(target);
            let label = format!("{} ({})", target.name, locale.t(status));
            html! { <option value={id.clone()} selected={id == form.target}>{label}</option> }
        })
        .collect::<Html>();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.target")}</label>
            { select_view(form, options, |f, value| f.target = value.to_string()) }
        </>
    }
}

fn parse_kind(value: &str) -> SourceKind {
    KINDS
        .iter()
        .find(|(_, item, _)| *item == value)
        .map_or_else(SourceKind::default, |(kind, _, _)| *kind)
}

fn parse_restart(value: &str) -> RestartPolicy {
    RESTARTS
        .iter()
        .find(|(_, item, _)| *item == value)
        .map_or_else(RestartPolicy::default, |(restart, _, _)| *restart)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    fn image_form() -> AppForm {
        AppForm {
            name: " shop ".into(),
            image: "nginx:1.27".into(),
            port: "8080".into(),
            domains: "shop.example.com\n\n www.example.com ".into(),
            volumes: "shop-data:/data\nshop-cache:/cache:ro".into(),
            restart: RestartPolicy::Always,
            memory_mb: "512".into(),
            cpus: "1.5".into(),
            ..AppForm::default()
        }
    }

    #[test]
    fn an_image_form_makes_the_request_of_the_admin_api() {
        let request = image_form().parse(Locale::En).unwrap();
        assert_eq!(
            to_value(&request).unwrap(),
            json!({
                "name": "shop",
                "target": "local",
                "spec": {
                    "source": {"type": "image", "image": "nginx:1.27"},
                    "port": 8080,
                    "domains": ["shop.example.com", "www.example.com"],
                    "volumes": [
                        {"volume": "shop-data", "target": "/data", "read_only": false},
                        {"volume": "shop-cache", "target": "/cache", "read_only": true}
                    ],
                    "restart": "always",
                    "limits": {"memory_bytes": 536_870_912_u64, "nano_cpus": 1_500_000_000_u64}
                }
            })
        );
    }

    #[test]
    fn a_compose_form_hides_the_container_settings() {
        let form = AppForm {
            kind: SourceKind::Compose,
            repository: "https://github.com/owner/shop.git".into(),
            service: "web".into(),
            ..image_form()
        };
        let request = form.parse(Locale::En).unwrap();
        assert!(request.spec.volumes.is_empty());
        assert_eq!(request.spec.restart, RestartPolicy::default());
        assert_eq!(request.spec.limits, ResourceLimits::default());
        let AppSource::Compose { file, branch, .. } = &request.spec.source else {
            panic!("expected a Compose source: {:?}", request.spec.source);
        };
        assert_eq!((file, branch.as_str()), (&None, "main"));
    }

    #[test]
    fn an_existing_app_fills_the_form_again() {
        let request = image_form().parse(Locale::En).unwrap();
        let app = AppEntry {
            id: "shop".parse().unwrap(),
            name: request.name.clone(),
            target: request.target,
            spec: request.spec.clone(),
            git_token_set: false,
            webhook_secret_set: false,
            created_at: 0,
            updated_at: 0,
        };
        let form = AppForm::edit(&app);
        assert_eq!(form.parse(Locale::En).unwrap(), request);
        assert_eq!(form.cpus, "1.5");
        assert!(!form.uses_git());
    }

    #[test]
    fn invalid_values_have_translated_errors() {
        let cases = [
            (
                AppForm {
                    port: "0".into(),
                    ..image_form()
                },
                "apps.invalid_port",
            ),
            (
                AppForm {
                    memory_mb: "-1".into(),
                    ..image_form()
                },
                "apps.invalid_memory",
            ),
            (
                AppForm {
                    cpus: "0".into(),
                    ..image_form()
                },
                "apps.invalid_cpus",
            ),
            (
                AppForm {
                    target: String::new(),
                    ..image_form()
                },
                "apps.invalid_target",
            ),
        ];
        for (form, key) in cases {
            assert_eq!(form.parse(Locale::Tr), Err(Locale::Tr.t(key).to_string()));
        }
        let volume = AppForm {
            volumes: "data".into(),
            ..image_form()
        };
        assert_eq!(
            volume.parse(Locale::En),
            Err(Locale::En.tf("apps.invalid_volume", &[("volume", "data")]))
        );
        let name = AppForm {
            name: "Shop".into(),
            ..image_form()
        };
        assert!(name.parse(Locale::En).is_err());
    }

    #[test]
    fn the_selects_read_their_own_values() {
        for (kind, value, _) in KINDS {
            assert_eq!(parse_kind(value), kind);
        }
        for (restart, value, _) in RESTARTS {
            assert_eq!(parse_restart(value), restart);
        }
        assert_eq!(parse_kind("other"), SourceKind::Image);
    }
}
