use std::{env, fs, io, path::PathBuf};

use ratatui::layout::Rect;

const MAX_LIMIT: usize = 12;
const MAX_GAP: usize = 3;
pub const FIELD_NAMES: [&str; 5] = [
    "Session columns",
    "Session rows",
    "Window columns",
    "Window rows",
    "Card gap",
];

pub fn field_range(field: usize) -> std::ops::RangeInclusive<usize> {
    if field == 4 {
        0..=MAX_GAP
    } else {
        1..=MAX_LIMIT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub columns: usize,
    pub rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub sessions: Limits,
    pub windows: Limits,
    pub gap: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sessions: Limits {
                columns: 2,
                rows: 2,
            },
            windows: Limits {
                columns: 2,
                rows: 2,
            },
            gap: 1,
        }
    }
}

impl Settings {
    fn path() -> io::Result<PathBuf> {
        let config = env::var_os("XDG_CONFIG_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No configuration directory"))?;
        Ok(config.join("tmux-expose").join("grid.conf"))
    }

    pub fn load() -> io::Result<Self> {
        let data = match fs::read_to_string(Self::path()?) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        Self::parse(&data)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid grid settings"))
    }

    pub fn parse(data: &str) -> Option<Self> {
        let fields: Vec<_> = data
            .split_whitespace()
            .map(str::parse::<usize>)
            .collect::<Result<_, _>>()
            .ok()?;
        match *fields.as_slice() {
            [sc, sr, wc, wr] => Self::from_fields([sc, sr, wc, wr, 1]),
            [sc, sr, wc, wr, gap] => Self::from_fields([sc, sr, wc, wr, gap]),
            _ => None,
        }
    }

    fn from_fields([sc, sr, wc, wr, gap]: [usize; 5]) -> Option<Self> {
        [sc, sr, wc, wr, gap]
            .iter()
            .enumerate()
            .all(|(field, n)| field_range(field).contains(n))
            .then_some(Self {
                sessions: Limits {
                    columns: sc,
                    rows: sr,
                },
                windows: Limits {
                    columns: wc,
                    rows: wr,
                },
                gap: gap as u16,
            })
    }

    pub fn fields(self) -> [usize; 5] {
        [
            self.sessions.columns,
            self.sessions.rows,
            self.windows.columns,
            self.windows.rows,
            self.gap.into(),
        ]
    }

    pub fn encode(self) -> String {
        let [sc, sr, wc, wr, gap] = self.fields();
        format!("{sc} {sr} {wc} {wr} {gap}")
    }

    pub fn save(self) -> io::Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, format!("{}\n", self.encode()))
    }
}

#[derive(Debug)]
pub struct SettingsEdit {
    pub field: usize,
    pub values: [String; 5],
    replace: bool,
}

impl SettingsEdit {
    pub fn new(settings: Settings) -> Self {
        Self {
            field: 0,
            values: settings.fields().map(|value| value.to_string()),
            replace: true,
        }
    }

    pub fn select(&mut self, field: usize) {
        self.field = field;
        self.replace = true;
    }

    pub fn digit(&mut self, ch: char) {
        let value = &mut self.values[self.field];
        if std::mem::take(&mut self.replace) {
            value.clear();
        }
        if value.len() < 3 {
            value.push(ch);
        }
    }

    pub fn backspace(&mut self) {
        self.replace = false;
        self.values[self.field].pop();
    }

    pub fn adjust(&mut self, increase: bool) {
        let value = &mut self.values[self.field];
        let range = field_range(self.field);
        let (min, max) = (*range.start(), *range.end());
        let old = value.parse::<usize>().unwrap_or(min);
        let next = if increase {
            old + 1
        } else {
            old.saturating_sub(1)
        };
        *value = next.clamp(min, max).to_string();
        self.replace = true;
    }

    pub fn settings(&self) -> Option<Settings> {
        Settings::from_fields([
            self.values[0].parse().ok()?,
            self.values[1].parse().ok()?,
            self.values[2].parse().ok()?,
            self.values[3].parse().ok()?,
            self.values[4].parse().ok()?,
        ])
    }

    pub fn encode(&self) -> String {
        format!(
            "{} {} {}",
            self.field,
            u8::from(self.replace),
            self.values.join(",")
        )
    }

    pub fn decode(data: &str) -> Option<Self> {
        let mut parts = data.splitn(3, ' ');
        let field: usize = parts.next()?.parse().ok()?;
        let replace = parts.next()? == "1";
        let values: [String; 5] = parts
            .next()?
            .split(',')
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .try_into()
            .ok()?;
        (field < values.len()
            && values
                .iter()
                .all(|s| s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit())))
        .then_some(Self {
            field,
            values,
            replace,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub columns: usize,
    rows: usize,
    pub first_row: usize,
    count: usize,
}

pub(crate) fn grid_cell(area: Rect, index: usize, columns: usize, rows: usize) -> Rect {
    let col = index % columns;
    let row = index / columns;
    let x = |i| (i * usize::from(area.width) / columns) as u16;
    let y = |i| (i * usize::from(area.height) / rows) as u16;
    Rect::new(
        area.x + x(col),
        area.y + y(row),
        x(col + 1) - x(col),
        y(row + 1) - y(row),
    )
}

impl Viewport {
    pub fn new(
        area: Rect,
        limits: Limits,
        minimum: (u16, u16),
        count: usize,
        selected: usize,
        first_row: usize,
    ) -> Self {
        let columns = limits
            .columns
            .min(usize::from(area.width / minimum.0).max(1))
            .min(count.max(1));
        let total_rows = count.div_ceil(columns).max(1);
        let rows = limits
            .rows
            .min(usize::from(area.height / minimum.1).max(1))
            .min(total_rows);
        let selected_row = selected.min(count.saturating_sub(1)) / columns;
        let first_row = first_row
            .min(total_rows - rows)
            .clamp((selected_row + 1).saturating_sub(rows), selected_row);
        Self {
            columns,
            rows,
            first_row,
            count,
        }
    }

    pub fn range(self) -> std::ops::Range<usize> {
        self.first_row * self.columns..((self.first_row + self.rows) * self.columns).min(self.count)
    }

    pub fn cards(self, area: Rect, gap: u16) -> impl Iterator<Item = (usize, Rect)> {
        self.range().enumerate().map(move |(slot, index)| {
            let col = slot % self.columns;
            let row = slot / self.columns;
            let cell = grid_cell(area, slot, self.columns, self.rows);
            let gap_x = if col + 1 < self.columns {
                gap.min(cell.width.saturating_sub(4))
            } else {
                0
            };
            let gap_y = if row + 1 < self.rows {
                gap.min(cell.height.saturating_sub(4))
            } else {
                0
            };
            (
                index,
                Rect::new(cell.x, cell.y, cell.width - gap_x, cell.height - gap_y),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_configuration_keeps_the_original_gap() {
        assert_eq!(
            Settings::parse("2 3 4 5").unwrap().fields(),
            [2, 3, 4, 5, 1]
        );
        assert_eq!(Settings::parse("2 3 4 5 0").unwrap().gap, 0);
        assert!(Settings::parse("2 3 4 5 4").is_none());
    }

    #[test]
    fn oversize_input_does_not_save_as_its_valid_prefix() {
        let mut edit = SettingsEdit::new(Settings::default());
        for ch in ['1', '2', '3'] {
            edit.digit(ch);
        }
        assert!(
            SettingsEdit::decode(&edit.encode())
                .unwrap()
                .settings()
                .is_none()
        );
        edit.backspace();
        assert_eq!(edit.settings().unwrap().sessions.columns, 12);
    }

    #[test]
    fn two_by_two_slides_a_row_and_reflows_to_one_column_when_narrow() {
        let wide = Rect::new(0, 0, 100, 40);
        let limits = Settings::default().sessions;
        let viewport =
            |area, selected, row| Viewport::new(area, limits, (32, 10), 7, selected, row);
        assert_eq!(viewport(wide, 0, 0).range(), 0..4);
        assert_eq!(viewport(wide, 4, 0).range(), 2..6);
        assert_eq!(viewport(wide, 6, 1).range(), 4..7);
        let narrow = Rect::new(0, 0, 38, 30);
        let v = viewport(narrow, 6, 2);
        assert_eq!(v.range(), 5..7);
        assert!(
            v.cards(narrow, 1)
                .all(|(_, card)| narrow.intersection(card) == card)
        );
    }
}
