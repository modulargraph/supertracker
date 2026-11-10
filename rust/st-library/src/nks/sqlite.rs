//! A minimal read-only reader for one table of an SQLite database file.
//!
//! Native Instruments keeps product background colours in a small SQLite file
//! (`NI Resources/_DatabaseResources/color/color.db`). Reading its one table does not justify
//! linking SQLite, so this module walks the table's B-tree directly, following the documented
//! file format (https://www.sqlite.org/fileformat2.html). It reads rowid tables in UTF-8
//! databases, including overflow pages; everything is bounded and any inconsistency ends the
//! read with what was found so far.

/// One column value of a row.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// NULL.
    Null,
    /// An integer.
    Integer(i64),
    /// A float.
    Real(f64),
    /// Text.
    Text(String),
    /// A blob.
    Blob(Vec<u8>),
}

impl Value {
    /// The text of a text value.
    pub fn text(&self) -> Option<&str> {
        match self {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// A table of a database: its column names (lower case) and rows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    /// Column names in declaration order, lower case.
    pub columns: Vec<String>,
    /// Rows in B-tree order.
    pub rows: Vec<Vec<Value>>,
}

struct Database<'a> {
    bytes: &'a [u8],
    page_size: usize,
    usable: usize,
    visited: usize,
}

fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for index in 0..9 {
        let byte = *bytes.get(*at)?;
        *at += 1;
        if index == 8 {
            return Some(value << 8 | u64::from(byte));
        }
        value = value << 7 | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

impl<'a> Database<'a> {
    fn open(bytes: &'a [u8]) -> Option<Database<'a>> {
        if bytes.len() < 100 || &bytes[0..16] != b"SQLite format 3\0" {
            return None;
        }
        let raw = u16::from_be_bytes([bytes[16], bytes[17]]) as usize;
        let page_size = if raw == 1 { 65_536 } else { raw };
        if page_size < 512 || !page_size.is_power_of_two() {
            return None;
        }
        // Only UTF-8 databases (text encoding 1) are read.
        if u32::from_be_bytes([bytes[56], bytes[57], bytes[58], bytes[59]]) != 1 {
            return None;
        }
        let usable = page_size.checked_sub(bytes[20] as usize)?;
        Some(Database { bytes, page_size, usable, visited: 0 })
    }

    fn page(&self, number: u32) -> Option<&'a [u8]> {
        let start = (number as usize).checked_sub(1)?.checked_mul(self.page_size)?;
        self.bytes.get(start..start + self.page_size)
    }

    /// The payload of a table leaf cell, following overflow pages.
    fn payload(&self, page: &'a [u8], mut at: usize) -> Option<Vec<u8>> {
        let size = varint(page, &mut at)? as usize;
        varint(page, &mut at)?; // rowid
        if size > 16 * 1024 * 1024 {
            return None;
        }
        let max_local = self.usable - 35;
        if size <= max_local {
            return page.get(at..at + size).map(<[u8]>::to_vec);
        }
        let min_local = (self.usable - 12) * 32 / 255 - 23;
        let mut local = min_local + (size - min_local) % (self.usable - 4);
        if local > max_local {
            local = min_local;
        }
        let mut result = page.get(at..at + local)?.to_vec();
        let pointer = page.get(at + local..at + local + 4)?;
        let mut next = u32::from_be_bytes([pointer[0], pointer[1], pointer[2], pointer[3]]);
        let mut hops = 0;
        while next != 0 && result.len() < size {
            hops += 1;
            if hops > 65_536 {
                return None;
            }
            let overflow = self.page(next)?;
            next = u32::from_be_bytes([overflow[0], overflow[1], overflow[2], overflow[3]]);
            let take = (size - result.len()).min(self.usable - 4);
            result.extend_from_slice(overflow.get(4..4 + take)?);
        }
        (result.len() == size).then_some(result)
    }

    /// Collects the rows of the table B-tree rooted at `root`, up to `limit`.
    fn rows(&mut self, root: u32, limit: usize, rows: &mut Vec<Vec<Value>>) -> Option<()> {
        self.visited += 1;
        if self.visited > 100_000 || rows.len() >= limit {
            return Some(());
        }
        let page = self.page(root)?;
        let header = if root == 1 { 100 } else { 0 };
        let kind = *page.get(header)?;
        let cells = u16::from_be_bytes([page[header + 3], page[header + 4]]) as usize;
        let pointers = header + if kind == 0x05 { 12 } else { 8 };
        let cell = |index: usize| -> Option<usize> {
            let at = pointers + index * 2;
            Some(u16::from_be_bytes([*page.get(at)?, *page.get(at + 1)?]) as usize)
        };
        match kind {
            0x0d => {
                for index in 0..cells {
                    if rows.len() >= limit {
                        break;
                    }
                    if let Some(record) = self.payload(page, cell(index)?).and_then(|payload| record(&payload)) {
                        rows.push(record);
                    }
                }
            }
            0x05 => {
                for index in 0..cells {
                    let at = cell(index)?;
                    let child = page.get(at..at + 4)?;
                    self.rows(u32::from_be_bytes([child[0], child[1], child[2], child[3]]), limit, rows)?;
                }
                let right = page.get(header + 8..header + 12)?;
                self.rows(u32::from_be_bytes([right[0], right[1], right[2], right[3]]), limit, rows)?;
            }
            _ => return None,
        }
        Some(())
    }
}

/// Decodes a record: a header of serial types followed by the values.
fn record(payload: &[u8]) -> Option<Vec<Value>> {
    let mut at = 0;
    let header = varint(payload, &mut at)? as usize;
    if header > payload.len() {
        return None;
    }
    let mut types = Vec::new();
    while at < header {
        types.push(varint(payload, &mut at)?);
    }
    let mut body = header;
    let mut values = Vec::with_capacity(types.len());
    for serial in types {
        let mut take = |count: usize| -> Option<&[u8]> {
            let slice = payload.get(body..body + count)?;
            body += count;
            Some(slice)
        };
        values.push(match serial {
            0 => Value::Null,
            1..=6 => {
                let width = [1, 2, 3, 4, 6, 8][serial as usize - 1];
                let bytes = take(width)?;
                let mut value = if bytes[0] & 0x80 != 0 { -1i64 } else { 0 };
                for byte in bytes {
                    value = value << 8 | i64::from(*byte);
                }
                Value::Integer(value)
            }
            7 => Value::Real(f64::from_bits(u64::from_be_bytes(take(8)?.try_into().ok()?))),
            8 => Value::Integer(0),
            9 => Value::Integer(1),
            10 | 11 => return None,
            serial if serial % 2 == 0 => Value::Blob(take((serial as usize - 12) / 2)?.to_vec()),
            serial => Value::Text(String::from_utf8_lossy(take((serial as usize - 13) / 2)?).into_owned()),
        });
    }
    Some(values)
}

/// The column names of a `CREATE TABLE` statement, lower case.
fn columns(sql: &str) -> Vec<String> {
    let Some(open) = sql.find('(') else { return Vec::new() };
    let inner = &sql[open + 1..sql.rfind(')').unwrap_or(sql.len())];
    let (mut depth, mut current, mut parts) = (0, String::new(), Vec::new());
    for character in inner.chars() {
        match character {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(character);
    }
    parts.push(current);
    parts
        .iter()
        .filter_map(|part| part.split_whitespace().next())
        .map(|name| name.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']' || c == '\'').to_lowercase())
        .filter(|name| !["primary", "unique", "constraint", "foreign", "check"].contains(&name.as_str()))
        .collect()
}

/// Reads up to `limit` rows of the table `name` from database file bytes. `None` when the
/// bytes are not a readable SQLite database or the table does not exist.
pub fn read_table(bytes: &[u8], name: &str, limit: usize) -> Option<Table> {
    let mut database = Database::open(bytes)?;
    let mut schema = Vec::new();
    database.rows(1, 10_000, &mut schema)?;
    let entry = schema.iter().find(|row| row.first().and_then(Value::text) == Some("table") && row.get(1).and_then(Value::text).is_some_and(|table| table.eq_ignore_ascii_case(name)))?;
    let root = match entry.get(3)? {
        Value::Integer(root) if *root > 0 => *root as u32,
        _ => return None,
    };
    let mut table = Table { columns: columns(entry.get(4).and_then(Value::text).unwrap_or("")), rows: Vec::new() };
    database.visited = 0;
    database.rows(root, limit, &mut table.rows);
    Some(table)
}
