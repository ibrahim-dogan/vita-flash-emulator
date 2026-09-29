//! The Explore catalog: the Silvergames Flash archive's directory listing,
//! parsed into entries and cached on the memory card, plus what we've
//! learned about individual games (header info, whether a cover exists).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::platform;
use crate::swfinfo::SwfInfo;

/// Where the games come from.
pub const SOURCE_NAME: &str = "Silvergames";
pub const SOURCE_HOST: &str = "files.silvergames.com";
const LISTING_URL: &str = "http://files.silvergames.com/flash/";

pub fn listing_url() -> &'static str {
    LISTING_URL
}

pub fn swf_url(slug: &str) -> String {
    format!("{LISTING_URL}{slug}.swf")
}

/// Cover art for games still listed on silvergames.com (about one in six).
pub fn cover_url(slug: &str) -> String {
    format!("http://media.silvergames.com/j/b/s/{slug}.jpg")
}

/// Refetch the listing after this long.
pub const MAX_AGE_SECS: i64 = 7 * 24 * 3600;

#[derive(Clone, Debug)]
pub struct Entry {
    /// File name without ".swf", e.g. "raft-wars-2".
    pub slug: String,
    pub title: String,
    /// Title in lower case, for sorting.
    lower: String,
    /// Title and file name in lower case, for searching.
    pub search: String,
    /// Approximate: the listing rounds to e.g. "3.8M".
    pub size: u64,
    /// Upload date, "YYYY-MM-DD".
    pub date: String,
}

impl Entry {
    fn new(slug: String, size: u64, date: String) -> Self {
        let title = title_from_slug(&slug);
        let lower = title.to_lowercase();
        let search = format!("{lower} {}", slug.replace(['-', '_'], " "));
        Entry { slug, title, lower, search, size, date }
    }

    pub fn year(&self) -> &str {
        self.date.get(..4).unwrap_or("")
    }

    /// The file name it's saved as in the games folder.
    pub fn file_name(&self) -> String {
        let safe: String = self
            .title
            .chars()
            .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
            .collect();
        format!("{safe}.swf")
    }
}

/// "raft-wars-2" -> "Raft Wars 2", "stick-war_v2" -> "Stick War (v2)".
pub fn title_from_slug(slug: &str) -> String {
    let (base, version) = match slug.rsplit_once("_v") {
        Some((b, v)) if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()) => (b, Some(v)),
        _ => (slug, None),
    };
    const SMALL: [&str; 11] = ["a", "an", "and", "at", "by", "for", "in", "of", "on", "the", "to"];
    let words: Vec<String> = base
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .enumerate()
        .map(|(i, w)| {
            if i > 0 && SMALL.contains(&w) {
                return w.to_owned();
            }
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect();
    let mut title = words.join(" ");
    if let Some(v) = version {
        title.push_str(&format!(" (v{v})"));
    }
    title
}

fn parse_size(s: &str) -> u64 {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('K') => (&s[..s.len() - 1], 1024.0),
        Some('M') => (&s[..s.len() - 1], 1024.0 * 1024.0),
        Some('G') => (&s[..s.len() - 1], 1024.0 * 1024.0 * 1024.0),
        _ => (s, 1.0),
    };
    (num.trim().parse::<f64>().unwrap_or(0.0) * mult) as u64
}

/// Pulls `.swf` rows out of an Apache "Index of" page.
pub fn parse_listing(html: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for row in html.split("<tr>").skip(1) {
        let Some(href_at) = row.find("<a href=\"") else { continue };
        let rest = &row[href_at + 9..];
        let Some(end) = rest.find('"') else { continue };
        let href = &rest[..end];
        let Some(slug) = href.strip_suffix(".swf") else { continue };
        if slug.contains('/') || slug.contains('?') || slug.is_empty() {
            continue;
        }
        // The next two right-aligned cells are the date and the size.
        let cells: Vec<&str> = rest
            .split("<td align=\"right\">")
            .skip(1)
            .map(|c| c.split('<').next().unwrap_or("").trim())
            .collect();
        let date = cells.first().map(|d| d.split_whitespace().next().unwrap_or("").to_owned()).unwrap_or_default();
        let size = cells.get(1).map(|s| parse_size(s)).unwrap_or(0);
        out.push(Entry::new(slug.to_owned(), size, date));
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Sort {
    #[default]
    Name,
    Newest,
    Smallest,
}

impl Sort {
    pub fn next(self) -> Sort {
        match self {
            Sort::Name => Sort::Newest,
            Sort::Newest => Sort::Smallest,
            Sort::Smallest => Sort::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sort::Name => "Sort: A\u{2013}Z",
            Sort::Newest => "Sort: Newest",
            Sort::Smallest => "Sort: Smallest",
        }
    }
}

pub struct Catalog {
    pub entries: Vec<Entry>,
    /// Unix time the listing was downloaded.
    pub fetched: i64,
}

fn dir() -> PathBuf {
    platform::data_dir().join("explore")
}

fn cache_path() -> PathBuf {
    dir().join("catalog.tsv")
}

pub fn covers_dir() -> PathBuf {
    dir().join("covers")
}

impl Catalog {
    pub fn load_cached() -> Option<Catalog> {
        let text = std::fs::read_to_string(cache_path()).ok()?;
        let mut lines = text.lines();
        let fetched = lines.next()?.strip_prefix("# fetched ")?.trim().parse().ok()?;
        let entries: Vec<Entry> = lines
            .filter_map(|l| {
                let mut f = l.split('\t');
                let (slug, size, date) = (f.next()?, f.next()?.parse().ok()?, f.next()?);
                Some(Entry::new(slug.to_owned(), size, date.to_owned()))
            })
            .collect();
        (!entries.is_empty()).then_some(Catalog { entries, fetched })
    }

    pub fn save(&self) {
        let mut out = format!("# fetched {}\n", self.fetched);
        for e in &self.entries {
            out.push_str(&format!("{}\t{}\t{}\n", e.slug, e.size, e.date));
        }
        let _ = std::fs::create_dir_all(dir());
        if let Err(e) = std::fs::write(cache_path(), out) {
            tracing::warn!("Couldn't save the Explore catalog: {e}");
        }
    }

    pub fn stale(&self) -> bool {
        platform::now_unix() - self.fetched > MAX_AGE_SECS
    }

    /// Indices of the entries matching `query` (all words, any order).
    /// Titles starting with the query come first, then titles where every
    /// word starts a word, then the rest ("raft" also finds "Minecraft");
    /// `sort` orders each group.
    pub fn view(&self, query: &str, sort: Sort) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        let rank = |e: &Entry| -> Option<u8> {
            if !words.iter().all(|w| e.search.contains(w)) {
                return None;
            }
            if words.is_empty() || e.search.starts_with(q.as_str()) {
                return Some(0);
            }
            let starts_word = |w: &str| e.search.split(' ').any(|t| t.starts_with(w));
            Some(if words.iter().all(|w| starts_word(w)) { 1 } else { 2 })
        };
        let mut v: Vec<(u8, usize)> = (0..self.entries.len())
            .filter_map(|i| rank(&self.entries[i]).map(|r| (r, i)))
            .collect();
        let e = &self.entries;
        v.sort_by(|&(ra, a), &(rb, b)| {
            ra.cmp(&rb).then_with(|| match sort {
                Sort::Name => e[a].lower.cmp(&e[b].lower),
                Sort::Newest => e[b].date.cmp(&e[a].date).then(a.cmp(&b)),
                Sort::Smallest => e[a].size.cmp(&e[b].size).then(a.cmp(&b)),
            })
        });
        v.into_iter().map(|(_, i)| i).collect()
    }
}

/// What a quick look at one game found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Probe {
    /// Header info from the first few KB, or why it couldn't be read.
    pub info: Option<Result<SwfInfo, String>>,
    /// Exact size, from the server.
    pub total: Option<u64>,
    /// Whether silvergames.com has cover art (saved in `covers_dir`).
    pub has_cover: bool,
}

fn probes_path() -> PathBuf {
    dir().join("probes.ron")
}

pub fn load_probes() -> HashMap<String, Probe> {
    std::fs::read_to_string(probes_path())
        .ok()
        .and_then(|t| ron::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_probes(probes: &HashMap<String, Probe>) {
    let _ = std::fs::create_dir_all(dir());
    match ron::to_string(probes) {
        Ok(t) => {
            let _ = std::fs::write(probes_path(), t);
        }
        Err(e) => tracing::warn!("Couldn't save Explore details: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title_from_slug("raft-wars-2"), "Raft Wars 2");
        assert_eq!(title_from_slug("stick-war_v2"), "Stick War (v2)");
        assert_eq!(title_from_slug("1-on-1-soccer"), "1 on 1 Soccer");
        assert_eq!(title_from_slug("papa-louie-2_nomo"), "Papa Louie 2 Nomo");
    }

    #[test]
    fn search_ranks_word_starts_first() {
        let c = Catalog {
            entries: ["2d-minecraft", "raft-wars-2", "aircraft-race", "raft-wars", "mega-raft"]
                .iter()
                .map(|s| Entry::new(s.to_string(), 1, "2013-01-01".into()))
                .collect(),
            fetched: 0,
        };
        let titles: Vec<&str> = c.view("raft", Sort::Name).iter().map(|&i| c.entries[i].title.as_str()).collect();
        assert_eq!(titles, ["Raft Wars", "Raft Wars 2", "Mega Raft", "2d Minecraft", "Aircraft Race"]);
    }

    #[test]
    fn parses_apache_rows() {
        let html = r#"<tr><td valign="top">&nbsp;</td><td><a href="/">Parent Directory</a></td><td>&nbsp;</td><td align="right">  - </td></tr>
<tr><td valign="top">&nbsp;</td><td><a href="1-on-1-soccer-brazil.swf">1-on-1-soccer-brazil..&gt;</a></td><td align="right">2013-12-11 08:08  </td><td align="right">5.5M</td><td>&nbsp;</td></tr>
<tr><td valign="top">&nbsp;</td><td><a href="bowman.swf">bowman.swf</a></td><td align="right">2008-11-20 09:01  </td><td align="right"> 34K</td><td>&nbsp;</td></tr>
<tr><td valign="top">&nbsp;</td><td><a href="dosbox/">dosbox/</a></td><td align="right">2025-10-21 07:18  </td><td align="right">  - </td></tr>"#;
        let e = parse_listing(html);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].slug, "1-on-1-soccer-brazil");
        assert_eq!(e[0].date, "2013-12-11");
        assert_eq!(e[0].size, (5.5 * 1024.0 * 1024.0) as u64);
        assert_eq!(e[1].title, "Bowman");
        assert_eq!(e[1].size, 34 * 1024);
    }
}
