//! Inert parsing of the single-part YTD export. Only the two selected members
//! are decompressed. The inventory verifies their CRC, names and declared size.
use discord_archive::{ArchiveError as Error, ArchiveInventory, EntryIndex};
use serde::Deserialize;
use std::io::{Read, Seek};

const MAX_ACCOUNT: u64 = 64 * 1024;
const MAX_POSTS: u64 = 2 * 1024 * 1024;

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
pub(super) fn parse<R: Read + Seek>(
    inventory: &mut ArchiveInventory<R>,
) -> Result<(Account, Vec<Post>), Error> {
    let mut account = None;
    let mut tweets = None;
    for i in 0..inventory.entry_count() {
        let index = EntryIndex(i);
        match inventory.selected_name(index) {
            Ok("data/account.js") => account = Some(index),
            Ok("data/tweets.js") => tweets = Some(index),
            Ok(name)
                if name.starts_with("data/tweets-part")
                    || name.starts_with("data/tweet-part")
                    || name == "data/tweet.js" =>
            {
                return Err(Error::UnsupportedProfile);
            }
            _ => (),
        }
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
    let Posts(posts) = member(
        inventory,
        tweets.ok_or(Error::UnsupportedProfile)?,
        b"window.YTD.tweets.part0 = ",
        MAX_POSTS,
    )?;
    Ok((account.account, posts))
}

struct Posts(Vec<Post>);

impl<'de> Deserialize<'de> for Posts {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Rows;
        impl<'de> serde::de::Visitor<'de> for Rows {
            type Value = Posts;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a bounded list of archived posts")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut rows: A,
            ) -> Result<Posts, A::Error> {
                use serde::de::Error;
                let mut posts = vec![];
                for _ in 0..4096 {
                    let Some(TweetRow { tweet: post }) = rows.next_element()? else {
                        return Ok(Posts(posts));
                    };
                    if !valid_id(&post.id_str)
                        || post.id.as_ref().is_some_and(|id| id != &post.id_str)
                    {
                        return Err(A::Error::custom("invalid post identity"));
                    }
                    if post.retweeted
                        || post.retweeted_status.is_some()
                        || post.full_text.starts_with("RT @")
                    {
                        continue;
                    }
                    if post.full_text.len() > 64 * 1024 {
                        return Err(A::Error::custom("post exceeds size limit"));
                    }
                    posts.push(post);
                }
                if rows.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(A::Error::custom("too many archived posts"));
                }
                Ok(Posts(posts))
            }
        }
        deserializer.deserialize_seq(Rows)
    }
}
