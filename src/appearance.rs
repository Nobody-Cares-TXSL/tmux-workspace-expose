use std::{env, fs, io, process::Command};

use anyhow::{Result, bail};
use ratatui::style::Color;

use crate::tmux::{run_tmux, tmux_output};

#[derive(Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub english: &'static str,
    pub option: &'static str,
    default: &'static str,
}

pub const FIELDS: [Field; 20] = [
    Field {
        name: "启动键",
        english: "Launch key",
        option: "key",
        default: "M-e",
    },
    Field {
        name: "按键表",
        english: "Key table",
        option: "key-table",
        default: "root",
    },
    Field {
        name: "宽度",
        english: "Width",
        option: "width",
        default: "100%",
    },
    Field {
        name: "高度",
        english: "Height",
        option: "height",
        default: "100%",
    },
    Field {
        name: "停靠",
        english: "Anchor",
        option: "anchor",
        default: "center",
    },
    Field {
        name: "弹窗背景",
        english: "Popup background",
        option: "background",
        default: "default",
    },
    Field {
        name: "弹窗边框",
        english: "Popup border",
        option: "border",
        default: "default",
    },
    Field {
        name: "Vim 导航",
        english: "Vim navigation",
        option: "vim-keys",
        default: "off",
    },
    Field {
        name: "选中卡片",
        english: "Selected card",
        option: "selected-color",
        default: "yellow",
    },
    Field {
        name: "当前 Session",
        english: "Current session",
        option: "attached-color",
        default: "green",
    },
    Field {
        name: "普通卡片",
        english: "Other cards",
        option: "inactive-color",
        default: "white",
    },
    Field {
        name: "Agent 需处理",
        english: "Agent needs attention",
        option: "attention-color",
        default: "#ff5555",
    },
    Field {
        name: "Agent 等待",
        english: "Agent waiting",
        option: "waiting-color",
        default: "#ffb86c",
    },
    Field {
        name: "Agent 运行中",
        english: "Agent working",
        option: "working-color",
        default: "#8be9fd",
    },
    Field {
        name: "界面语言",
        english: "Language",
        option: "language",
        default: "zh",
    },
    Field {
        name: "Agent 优先排序",
        english: "Prioritize agents",
        option: "agent-sort",
        default: "on",
    },
    Field {
        name: "布局刷新（ms）",
        english: "Layout refresh (ms)",
        option: "topology-refresh",
        default: "1500",
    },
    Field {
        name: "后台 Pane 刷新（ms）",
        english: "Background pane refresh (ms)",
        option: "pane-refresh",
        default: "1500",
    },
    Field {
        name: "活跃 Pane 刷新（ms）",
        english: "Active pane refresh (ms)",
        option: "active-pane-refresh",
        default: "300",
    },
    Field {
        name: "每批最多抓取",
        english: "Captures per batch",
        option: "max-captures",
        default: "4",
    },
];

pub const LAUNCH_KEY: usize = 0;
pub const KEY_TABLE: usize = 1;
pub const POPUP_WIDTH: usize = 2;
pub const POPUP_HEIGHT: usize = 3;
pub const POPUP_ANCHOR: usize = 4;
pub const POPUP_BACKGROUND: usize = 5;
pub const POPUP_BORDER: usize = 6;
pub const VIM_KEYS: usize = 7;
pub const SELECTED_COLOR: usize = 8;
pub const ATTACHED_COLOR: usize = 9;
pub const INACTIVE_COLOR: usize = 10;
pub const ATTENTION_COLOR: usize = 11;
pub const WAITING_COLOR: usize = 12;
pub const WORKING_COLOR: usize = 13;
pub const LANGUAGE: usize = 14;
pub const AGENT_SORT: usize = 15;
pub const TOPOLOGY_REFRESH: usize = 16;
pub const PANE_REFRESH: usize = 17;
pub const ACTIVE_REFRESH: usize = 18;
pub const MAX_CAPTURES: usize = 19;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Zh,
    En,
}

impl Language {
    pub fn text<'a>(self, zh: &'a str, en: &'a str) -> &'a str {
        match self {
            Self::Zh => zh,
            Self::En => en,
        }
    }
}

fn number_range(field: usize) -> Option<std::ops::RangeInclusive<u64>> {
    match field {
        TOPOLOGY_REFRESH..=ACTIVE_REFRESH => Some(1..=60_000),
        MAX_CAPTURES => Some(1..=32),
        _ => None,
    }
}

pub fn parse_number(index: usize, value: &str) -> Result<u64, String> {
    let range = number_range(index).ok_or_else(|| "expected a numeric setting".to_owned())?;
    value
        .parse()
        .ok()
        .filter(|number| range.contains(number))
        .ok_or_else(|| {
            format!(
                "{} must be in {}–{}",
                FIELDS[index].option,
                range.start(),
                range.end()
            )
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance(pub [String; FIELDS.len()]);

impl Default for Appearance {
    fn default() -> Self {
        Self(FIELDS.map(|field| field.default.to_owned()))
    }
}

impl Appearance {
    pub fn load(mut fallback: Self) -> Result<Self> {
        if let Some(saved) = read_saved()? {
            for (field, value) in fallback.0.iter_mut().zip(saved) {
                if let Some(value) = value {
                    *field = value;
                }
            }
            fallback.validate()?;
        }
        Ok(fallback)
    }

    #[doc(hidden)]
    pub fn popup_spec(
        mut options: [String; POPUP_BORDER + 1],
    ) -> Result<[String; POPUP_BORDER + 1]> {
        let table = default_key_table(&options[LAUNCH_KEY]);
        if options[KEY_TABLE].is_empty() {
            options[KEY_TABLE] = table.into();
        }
        for (value, field) in options[..=POPUP_ANCHOR].iter_mut().zip(FIELDS) {
            if value.is_empty() {
                *value = field.default.into();
            }
        }
        if let Some(saved) = read_saved()? {
            for (index, (option, value)) in options.iter_mut().zip(saved).enumerate() {
                if let Some(value) = value {
                    match index {
                        POPUP_BACKGROUND | POPUP_BORDER => {
                            if !option.is_empty() {
                                option.push(',');
                            }
                            option.push_str(if index == POPUP_BACKGROUND {
                                "bg="
                            } else {
                                "fg="
                            });
                            option.push_str(&value);
                        }
                        _ => *option = value,
                    }
                }
            }
        }
        anyhow::ensure!(
            options.iter().all(|value| !value.contains(['\n', '\r'])),
            "Popup settings must fit on one line"
        );
        Ok(options)
    }

    pub fn from_tmux() -> Self {
        let mut appearance = Self::default();
        let format = FIELDS
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let name = match index {
                    POPUP_BACKGROUND => "style",
                    POPUP_BORDER => "border-style",
                    _ => field.option,
                };
                format!("#{{@tmux-expose-{name}}}")
            })
            .collect::<Vec<_>>()
            .join("\u{e000}");
        let Ok(output) = run_tmux(
            &["display-message", "-p", &format],
            "Could not read tmux appearance",
        ) else {
            return appearance;
        };
        let parts = output.trim_end_matches('\n').split('\u{e000}');
        for (index, value) in parts.clone().take(FIELDS.len()).enumerate() {
            if value.is_empty() {
                continue;
            }
            let value = match index {
                POPUP_BACKGROUND | POPUP_BORDER => {
                    let prefix = if index == POPUP_BACKGROUND {
                        "bg="
                    } else {
                        "fg="
                    };
                    value
                        .split(',')
                        .find_map(|part| part.strip_prefix(prefix))
                        .unwrap_or("default")
                }
                VIM_KEYS => {
                    if matches!(value, "on" | "true" | "1" | "yes") {
                        "on"
                    } else {
                        "off"
                    }
                }
                _ => value,
            };
            appearance.0[index] = value.to_owned();
        }
        if parts.clone().nth(KEY_TABLE) == Some("") {
            appearance.0[KEY_TABLE] = default_key_table(parts.clone().next().unwrap_or("")).into();
        }
        appearance
    }

    pub fn language(&self) -> Language {
        if self.0[LANGUAGE] == "en" {
            Language::En
        } else {
            Language::Zh
        }
    }

    pub fn number(&self, index: usize) -> u64 {
        parse_number(index, &self.0[index]).unwrap_or_else(|_| {
            FIELDS[index]
                .default
                .parse()
                .expect("numeric setting default")
        })
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_in(self.language())
    }

    fn validate_in(&self, lang: Language) -> Result<()> {
        let values = &self.0;
        let key = &values[LAUNCH_KEY];
        let table = &values[KEY_TABLE];
        let modified = key.strip_prefix("M-").or_else(|| key.strip_prefix("C-"));
        let letter = |value: &str| value.len() == 1 && value.as_bytes()[0].is_ascii_alphabetic();
        if !matches!(table.as_str(), "root" | "prefix")
            || !(modified.is_some_and(letter) || (table == "prefix" && letter(key)))
            || matches!(key.as_str(), "M-w" | "C-c")
        {
            bail!(
                "{}",
                lang.text(
                    "启动键用 M-e、C-e 或 prefix 下的单个字母；M-w 和 C-c 保留",
                    "Use M-e, C-e or one letter with prefix. M-w and C-c are reserved."
                )
            );
        }
        for value in &values[POPUP_WIDTH..=POPUP_HEIGHT] {
            let (digits, max) = value
                .strip_suffix('%')
                .map_or((value.as_str(), 1000), |v| (v, 100));
            if !digits.parse::<u16>().is_ok_and(|n| (1..=max).contains(&n)) {
                bail!(
                    "{}",
                    lang.text(
                        "宽高用 1–100% 或 1–1000 个字符格",
                        "Use 1–100% or 1–1000 cells for width and height."
                    )
                );
            }
        }
        if !ANCHORS.contains(&values[POPUP_ANCHOR].as_str()) {
            bail!(
                "{}",
                lang.text(
                    "停靠用 center、top、bottom、left 或 right",
                    "Anchor must be center, top, bottom, left or right."
                )
            );
        }
        if !matches!(values[VIM_KEYS].as_str(), "on" | "off") {
            bail!(
                "{}",
                lang.text("Vim 导航用 on 或 off", "Vim navigation must be on or off.")
            );
        }
        if !matches!(values[LANGUAGE].as_str(), "zh" | "en") {
            bail!(
                "{}",
                lang.text("语言用 zh 或 en", "Language must be zh or en.")
            );
        }
        if !matches!(values[AGENT_SORT].as_str(), "on" | "off") {
            bail!(
                "{}",
                lang.text(
                    "Agent 优先排序用 on 或 off",
                    "Agent priority must be on or off."
                )
            );
        }
        for index in TOPOLOGY_REFRESH..=MAX_CAPTURES {
            let range = number_range(index).expect("numeric settings range");
            if parse_number(index, &values[index]).is_err() {
                let field = FIELDS[index];
                bail!(
                    "{}: {}–{}",
                    lang.text(field.name, field.english),
                    range.start(),
                    range.end()
                );
            }
        }
        for index in (POPUP_BACKGROUND..=POPUP_BORDER).chain(SELECTED_COLOR..=WORKING_COLOR) {
            let value = &values[index];
            if !value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'#')
                || (parse_color(value).is_err() && !(index <= POPUP_BORDER && value == "default"))
            {
                let field = FIELDS[index];
                bail!(
                    "{}: {}",
                    lang.text(field.name, field.english),
                    lang.text(
                        "请输入颜色名、colour0–255 或 #RRGGBB",
                        "Use a color name, colour0–255 or #RRGGBB."
                    )
                );
            }
        }
        Ok(())
    }

    pub fn save(&self, previous: &Self) -> Result<()> {
        let lang = previous.language();
        self.validate_in(lang)?;
        let path = crate::config::path("appearance.conf")?;
        let prior = fs::read(&path).ok();
        let plugin = if self.0[..VIM_KEYS] != previous.0[..VIM_KEYS] {
            env::var_os("TMUX_EXPOSE_PLUGIN_PATH")
        } else {
            None
        };
        let changed_key = self.0[..=KEY_TABLE] != previous.0[..=KEY_TABLE];
        if changed_key
            && plugin.is_some()
            && binding(&self.0[KEY_TABLE], &self.0[LAUNCH_KEY])?.is_some_and(|line| !ours(&line))
        {
            bail!(
                "{}",
                lang.text(
                    "这个启动键已被 tmux 占用",
                    "This launch key is already bound in tmux."
                )
            );
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data: String = FIELDS
            .iter()
            .zip(&self.0)
            .enumerate()
            .map(|(index, (field, value))| {
                let value = if matches!(index, POPUP_BACKGROUND | POPUP_BORDER) {
                    popup_color(value)
                } else {
                    value.clone()
                };
                format!("{}={value}\n", field.option)
            })
            .collect();
        fs::write(&path, data)?;
        if let Some(plugin) = plugin {
            let output = env::current_exe().and_then(|binary| {
                Command::new("bash")
                    .env("TMUX_EXPOSE_POPUP_HELPER", binary)
                    .arg(&plugin)
                    .output()
            });
            if !output.as_ref().is_ok_and(|output| output.status.success()) {
                match prior {
                    Some(data) => fs::write(&path, data)?,
                    None => fs::remove_file(&path)?,
                }
                let detail = output
                    .map(|out| String::from_utf8_lossy(&out.stderr).into_owned())
                    .unwrap_or_else(|error| error.to_string());
                bail!(
                    "{}: {}",
                    lang.text("更新 tmux 绑定失败", "Could not reload tmux bindings"),
                    detail.trim()
                );
            }
            if changed_key
                && binding(&previous.0[KEY_TABLE], &previous.0[LAUNCH_KEY])?
                    .is_some_and(|line| ours(&line))
            {
                run_tmux(
                    &[
                        "unbind-key",
                        "-T",
                        &previous.0[KEY_TABLE],
                        &previous.0[LAUNCH_KEY],
                    ],
                    lang.text(
                        "移除旧启动键失败",
                        "Could not remove the previous launch key",
                    ),
                )?;
            }
        }
        Ok(())
    }
}

fn default_key_table(key: &str) -> &'static str {
    if key.is_empty() {
        FIELDS[KEY_TABLE].default
    } else {
        "prefix"
    }
}

fn read_saved() -> Result<Option<[Option<String>; FIELDS.len()]>> {
    let data = match fs::read_to_string(crate::config::path("appearance.conf")?) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut saved = std::array::from_fn(|_| None);
    for (option, value) in data.lines().filter_map(|line| line.split_once('=')) {
        if let Some(index) = FIELDS.iter().position(|field| field.option == option) {
            saved[index] = Some(value.to_owned());
        }
    }
    Ok(Some(saved))
}

fn binding(table: &str, key: &str) -> Result<Option<String>> {
    let output = tmux_output(
        &["list-keys", "-T", table, key],
        "Could not look up tmux binding",
    )?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned()))
}

fn ours(line: &str) -> bool {
    if line
        .split_once(" run-shell -b ")
        .is_some_and(|(_, command)| {
            command
                .trim_start_matches('"')
                .starts_with("TMUX_EXPOSE_BINDING=overview ")
        })
    {
        return true;
    }
    // Recognize bindings installed before the ownership marker was added.
    line.contains("tmux.expose.tmux") && line.contains("--popup #{q:client_name} 0 ")
}

const ANCHORS: [&str; 5] = ["center", "top", "bottom", "left", "right"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Grid,
    Popup,
    Colors,
    Behavior,
}

impl Page {
    pub fn step(self, backward: bool) -> Self {
        let pages = [Self::Grid, Self::Popup, Self::Colors, Self::Behavior];
        let index = self.index();
        pages[(index + if backward { pages.len() - 1 } else { 1 }) % pages.len()]
    }

    pub fn index(self) -> usize {
        match self {
            Self::Grid => 0,
            Self::Popup => 1,
            Self::Colors => 2,
            Self::Behavior => 3,
        }
    }

    pub fn fields(self) -> std::ops::Range<usize> {
        match self {
            Self::Grid => 0..crate::grid::FIELD_NAMES.len(),
            Self::Popup => LAUNCH_KEY..SELECTED_COLOR,
            Self::Colors => SELECTED_COLOR..LANGUAGE,
            Self::Behavior => LANGUAGE..FIELDS.len(),
        }
    }
}

#[derive(Debug)]
pub struct AppearanceEdit {
    pub draft: Appearance,
    pub field: usize,
    replace: bool,
}

impl AppearanceEdit {
    pub fn new(draft: Appearance) -> Self {
        Self {
            draft,
            field: 0,
            replace: true,
        }
    }
    pub fn select(&mut self, field: usize) {
        self.field = field;
        self.replace = true;
    }
    pub fn insert(&mut self, ch: char) {
        let value = &mut self.draft.0[self.field];
        if std::mem::take(&mut self.replace) {
            value.clear();
        }
        if value.len() < 32 {
            value.push(ch);
        }
    }
    pub fn backspace(&mut self) {
        self.replace = false;
        self.draft.0[self.field].pop();
    }
    pub fn snapshot(&self) -> String {
        format!(
            "{}\t{}\t{}",
            self.field,
            u8::from(self.replace),
            self.draft.0.join("\t")
        )
    }
    pub fn restore(data: &str) -> Option<Self> {
        let mut parts = data.split('\t');
        let field = parts.next()?.parse().ok()?;
        let replace = parts.next()? == "1";
        let values: [String; FIELDS.len()] = parts
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .try_into()
            .ok()?;
        (field < values.len()).then_some(Self {
            draft: Appearance(values),
            field,
            replace,
        })
    }
    pub fn adjust(&mut self, forward: bool) {
        let value = &mut self.draft.0[self.field];
        if matches!(self.field, POPUP_WIDTH | POPUP_HEIGHT) {
            let old = value.trim_end_matches('%').parse::<i32>().unwrap_or(100);
            *value = format!("{}%", (old + if forward { 5 } else { -5 }).clamp(1, 100));
        } else if let Some(range) = number_range(self.field) {
            let old = value.parse::<u64>().unwrap_or(*range.start());
            let step = if self.field == MAX_CAPTURES { 1 } else { 100 };
            let next = if forward {
                old.saturating_add(step)
            } else {
                old.saturating_sub(step)
            };
            *value = next.clamp(*range.start(), *range.end()).to_string();
        } else {
            let options: &[&str] = match self.field {
                LAUNCH_KEY => &["M-e", "M-s", "M-o"],
                KEY_TABLE => &["root", "prefix"],
                POPUP_ANCHOR => &ANCHORS,
                VIM_KEYS | AGENT_SORT => &["off", "on"],
                LANGUAGE => &["zh", "en"],
                _ => &[
                    "default", "black", "white", "cyan", "green", "yellow", "magenta", "red",
                    "blue", "gray",
                ],
            };
            let options = if (SELECTED_COLOR..LANGUAGE).contains(&self.field) {
                &options[1..]
            } else {
                options
            };
            let current = options.iter().position(|option| option == value);
            let index = current.map_or(if forward { 0 } else { options.len() - 1 }, |i| {
                (i + if forward { 1 } else { options.len() - 1 }) % options.len()
            });
            *value = options[index].to_owned();
        }
        self.replace = true;
    }
}

pub fn parse_color(value: &str) -> Result<Color, String> {
    if let Ok(color) = value.parse() {
        return Ok(color);
    }
    if let Some(index) = value
        .strip_prefix("colour")
        .or_else(|| value.strip_prefix("color"))
        .and_then(|digits| digits.parse::<u8>().ok())
    {
        return Ok(Color::Indexed(index));
    }
    Err(format!("invalid color {value:?}"))
}

fn popup_color(value: &str) -> String {
    match parse_color(value) {
        Ok(Color::Rgb(r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}"),
        Ok(Color::Indexed(n)) => format!("colour{n}"),
        Ok(Color::Reset) | Err(_) => "default".to_owned(),
        Ok(color) => format!(
            "colour{}",
            match color {
                Color::Black => 0,
                Color::Red => 1,
                Color::Green => 2,
                Color::Yellow => 3,
                Color::Blue => 4,
                Color::Magenta => 5,
                Color::Cyan => 6,
                Color::Gray => 7,
                Color::DarkGray => 8,
                Color::LightRed => 9,
                Color::LightGreen => 10,
                Color::LightYellow => 11,
                Color::LightBlue => 12,
                Color::LightMagenta => 13,
                Color::LightCyan => 14,
                Color::White => 15,
                _ => unreachable!("RGB, indexed and default colors are handled above"),
            }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_current_and_legacy_overview_bindings_only() {
        assert!(ours(
            r#"bind-key -T root M-e run-shell -b "TMUX_EXPOSE_BINDING=overview bash new-launcher --new-order""#
        ));
        assert!(ours(
            r#"bind-key -T root M-e run-shell -b "bash '/old/tmux.expose.tmux' --popup #{q:client_name} 0 -E tmux-expose""#
        ));
        assert!(!ours(
            r#"bind-key -T root M-w run-shell -b "bash '/old/tmux.expose.tmux' --popup #{q:client_name} 2 -E tmux-expose""#
        ));
        assert!(!ours(
            r#"bind-key -T root M-e run-shell -b "echo TMUX_EXPOSE_BINDING=overview other-command""#
        ));
    }

    #[test]
    fn behavior_values_are_validated_and_language_can_be_changed_with_arrows() {
        let mut edit = AppearanceEdit::new(Appearance::default());
        edit.select(LANGUAGE);
        edit.adjust(true);
        assert_eq!(edit.draft.language(), Language::En);
        assert!(edit.draft.validate().is_ok());
        edit.draft.0[MAX_CAPTURES] = "33".into();
        assert!(
            edit.draft
                .validate()
                .unwrap_err()
                .to_string()
                .contains("Captures per batch")
        );
    }

    #[test]
    fn accepts_and_rejects_popup_edits_before_binding() {
        let mut appearance = Appearance::default();
        appearance.0[LAUNCH_KEY] = "M-z".into();
        appearance.0[POPUP_WIDTH] = "85%".into();
        appearance.0[POPUP_ANCHOR] = "bottom".into();
        appearance.0[POPUP_BACKGROUND] = "LightBlue".into();
        assert!(appearance.validate().is_ok());
        assert_eq!(popup_color(&appearance.0[POPUP_BACKGROUND]), "colour12");
        appearance.0[POPUP_WIDTH] = "101%".into();
        assert!(appearance.validate().is_err());
        appearance.0[POPUP_WIDTH] = "100%".into();
        appearance.0[LAUNCH_KEY] = "M-w".into();
        assert!(appearance.validate().is_err());
    }
}
