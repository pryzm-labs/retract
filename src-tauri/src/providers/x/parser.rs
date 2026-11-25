//! Inert parsing of numbered YTD post arrays in ZIP inventory order-independent form.
use discord_archive::{ArchiveError as Error, ArchiveInventory, EntryIndex};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Seek},
};

const MAX_ACCOUNT: u64 = 64 * 1024;
const MIB: u64 = 1024 * 1024;
const MAX_PARTS: usize = 32;
const MAX_RAW_ROW: u64 = 2 * MIB;

#[derive(Deserialize)]
pub(super) struct Account {
    #[serde(rename = "accountId")]
    pub id: String,
    pub username: String,
}
#[derive(Deserialize)]
struct AccountRow {
    account: Account,
}

#[derive(Deserialize)]
struct TweetRow {
    tweet: Post,
}
#[derive(Deserialize)]
pub(super) struct Post {
    pub id_str: String,
    id: Option<String>,
    pub full_text: String,
    pub created_at: String,
    #[serde(default)]
    retweeted: bool,
    retweeted_status: Option<serde::de::IgnoredAny>,
}
pub(super) fn valid_id(id: &str) -> bool {
    !id.starts_with('0')
        && !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && id.parse::<u64>().is_ok()
}
pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 50
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn member<T: serde::de::DeserializeOwned, R: Read + Seek>(
    inventory: &mut ArchiveInventory<R>,
    index: EntryIndex,
    prefix: &[u8],
    limit: u64,
) -> Result<T, Error> {
    inventory.consume(index, |reader| {
        let mut header = vec![0; prefix.len()];
        reader
            .read_exact(&mut header)
            .map_err(|_| Error::InvalidProfile)?;
        if header != prefix {
            return Err(Error::UnsupportedProfile);
        }
        let mut bounded = reader.take(limit + 1);
        let result = serde_json::from_reader(std::io::BufReader::new(&mut bounded));
        if bounded.limit() == 0 {
            return Err(Error::LimitExceeded);
        }
        result.map_err(|_| Error::InvalidJson)
    })
}
pub(super) struct Layout {
    pub account: Account,
    pub parts: Vec<EntryIndex>,
}

pub(super) fn inspect<R: Read + Seek>(
    inventory: &mut ArchiveInventory<R>,
) -> Result<Layout, Error> {
    let mut account = None;
    let mut parts = BTreeMap::new();
    for i in 0..inventory.entry_count() {
        let index = EntryIndex(i);
        let number = match inventory.selected_name(index) {
            Ok("data/account.js") => {
                account = Some(index);
                continue;
            }
            Ok("data/tweets.js") => 0,
            Ok(name) if name.starts_with("data/tweets-part") => {
                let digits = name
                    .strip_prefix("data/tweets-part")
                    .and_then(|s| s.strip_suffix(".js"))
                    .ok_or(Error::UnsupportedProfile)?;
                if digits.starts_with('0')
                    || digits.is_empty()
                    || !digits.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(Error::UnsupportedProfile);
                }
                let n = digits.parse::<usize>().map_err(|_| Error::LimitExceeded)?;
                if n >= MAX_PARTS {
                    return Err(Error::LimitExceeded);
                }
                n
            }
            Ok(name) if name == "data/tweet.js" || name.starts_with("data/tweet-part") => {
                return Err(Error::UnsupportedProfile);
            }
            _ => continue,
        };
        if parts.insert(number, index).is_some() {
            return Err(Error::InvalidProfile);
        }
    }
    if parts.is_empty() || !parts.keys().copied().eq(0..parts.len()) {
        return Err(Error::UnsupportedProfile);
    }
    let [account]: [AccountRow; 1] = member(
        inventory,
        account.ok_or(Error::UnsupportedProfile)?,
        b"window.YTD.account.part0 = ",
        MAX_ACCOUNT,
    )?;
    if !valid_id(&account.account.id) || !valid_name(&account.account.username) {
        return Err(Error::InvalidProfile);
    }
    Ok(Layout {
        account: account.account,
        parts: parts.into_values().collect(),
    })
}

// Leave the next JSON token available for Serde. Only JSON whitespace is legal.
fn next_token(reader: &mut impl BufRead) -> Result<Option<u8>, Error> {
    loop {
        let bytes = reader.fill_buf().map_err(|_| Error::ReadFailure)?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let skip = bytes
            .iter()
            .take_while(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
            .count();
        if skip == 0 {
            return Ok(Some(bytes[0]));
        }
        reader.consume(skip);
    }
}

pub(super) fn visit_posts<R: Read + Seek>(
    inventory: &mut ArchiveInventory<R>,
    parts: &[EntryIndex],
    mut visit: impl FnMut(Post) -> Result<(), Error>,
) -> Result<(), Error> {
    let multipart = parts.len() > 1;
    let member_limit = if multipart { 128 * MIB } else { 2 * MIB };
    let mut remaining = if multipart { 250_000 } else { 4096 };
    for (number, &entry) in parts.iter().enumerate() {
        let prefix = format!("window.YTD.tweets.part{number} = ");
        inventory.consume(entry, |input| {
            let mut header = vec![0; prefix.len()];
            input
                .read_exact(&mut header)
                .map_err(|_| Error::InvalidProfile)?;
            if header != prefix.as_bytes() {
                return Err(Error::UnsupportedProfile);
            }
            let mut reader = BufReader::new(input.take(member_limit + 1));
            if next_token(&mut reader)? != Some(b'[') {
                return Err(Error::InvalidJson);
            }
            reader.consume(1);
            if next_token(&mut reader)? != Some(b']') {
                loop {
                    if remaining == 0 {
                        return Err(Error::LimitExceeded);
                    }
                    remaining -= 1;
                    // Decode one self-delimiting object, bounding even unknown fields
                    // before any individual row can consume the entire member budget.
                    if next_token(&mut reader)? != Some(b'{') {
                        return Err(Error::InvalidJson);
                    }
                    let mut row = (&mut reader).take(MAX_RAW_ROW + 1);
                    let result =
                        TweetRow::deserialize(&mut serde_json::Deserializer::from_reader(&mut row));
                    if row.limit() == 0 {
                        return Err(Error::LimitExceeded);
                    }
                    let post = result.map_err(|_| Error::InvalidJson)?.tweet;
                    if !valid_id(&post.id_str)
                        || post.id.as_ref().is_some_and(|id| id != &post.id_str)
                    {
                        return Err(Error::InvalidProfile);
                    }
                    if !post.retweeted && post.retweeted_status.is_none() {
                        if post.full_text.len() > 64 * 1024 {
                            return Err(Error::LimitExceeded);
                        }
                        visit(post)?;
                    }
                    match next_token(&mut reader)? {
                        Some(b',') => reader.consume(1),
                        Some(b']') => break,
                        _ => return Err(Error::InvalidJson),
                    }
                }
            }
            reader.consume(1);
            let trailing = next_token(&mut reader)?;
            if reader.into_inner().limit() == 0 {
                return Err(Error::LimitExceeded);
            }
            if trailing.is_some() {
                return Err(Error::InvalidJson);
            }
            Ok(())
        })?;
    }
    Ok(())
}
