use crate::{
    Error, MemberAccess, Params, Stats,
    aggregates::profile,
    analytics::{average, iso, ranked, rounded},
};
use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use yoink_db::{Row, query};

const HTTP_STOP: &str = "the a an in on at to of is it and or but for with это как что так все там уже мне его она они ещё был не да же вот то из он по до во от со при за над под для";
const COMMAND_STOP: &str = "the a an in on at to of is it and or but for with i you he she we they this that are was were be been have has had do does did will would could should my your his her its our their not no so if as by up from what who how when where which";
const BATCH: i64 = 256;

#[derive(Default)]
struct User {
    count: usize,
    first: Option<i64>,
    last: Option<i64>,
    length_count: usize,
    length_sum: usize,
    length_max: usize,
}
#[derive(Default)]
struct Bucket {
    count: usize,
    text: usize,
    users: BTreeSet<i64>,
}
#[derive(Default)]
struct Delays {
    // Second bins and extrema preserve formatted continuous medians without retaining reply samples.
    bins: BTreeMap<i64, (usize, i64, i64)>,
    count: usize,
    sum: i128,
}
impl Delays {
    fn add(&mut self, micros: i64) {
        let bin = self
            .bins
            .entry(micros / 1_000_000)
            .or_insert((0, micros, micros));
        bin.0 += 1;
        bin.1 = bin.1.min(micros);
        bin.2 = bin.2.max(micros);
        self.count += 1;
        self.sum += micros as i128;
    }
    fn formatted(&self) -> (String, String) {
        if self.count == 0 {
            return ("-".into(), "-".into());
        }
        let lo = (self.count - 1) / 2;
        let hi = self.count / 2;
        let mut n = 0;
        let mut a = None;
        let mut lower_bin = None;
        let mut b = 0;
        for (second, (count, min, max)) in &self.bins {
            if a.is_none() && n + count > lo {
                a = Some(*max);
                lower_bin = Some(*second);
            }
            if n + count > hi {
                b = if lower_bin == Some(*second) {
                    *second * 1_000_000
                } else {
                    *min
                };
                if lower_bin == Some(*second) {
                    a = Some(b);
                }
                break;
            }
            n += count;
        }
        (
            duration((self.sum as f64 / self.count as f64 / 1_000_000.0).round()),
            duration((a.unwrap_or(b) as f64 + b as f64) / 2_000_000.0),
        )
    }
}
fn duration(n: f64) -> String {
    let n = n as i64;
    if n < 60 {
        format!("{n}s")
    } else if n < 3600 {
        format!("{}m", n / 60)
    } else {
        format!("{}h {}m", n / 3600, n % 3600 / 60)
    }
}
fn random_below(seed: &mut u64, bound: u64) -> u64 {
    // Rejection avoids modulo bias in reservoir selection.
    let threshold = bound.wrapping_neg() % bound;
    loop {
        if *seed == 0 {
            *seed = 1;
        }
        *seed ^= *seed >> 12;
        *seed ^= *seed << 25;
        *seed ^= *seed >> 27;
        let value = seed.wrapping_mul(2685821657736338717);
        if value >= threshold {
            return value % bound;
        }
    }
}
fn mentions(body: &str) -> Vec<String> {
    body.split('@')
        .skip(1)
        .map(|s| {
            s.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|s| s.len() >= 4)
        .collect()
}
fn with(mut base: Value, extra: Value) -> Value {
    base.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    base
}
fn date(n: i64) -> Result<DateTime<Utc>, Error> {
    DateTime::from_timestamp_micros(n).ok_or(Error::Database)
}
async fn revision(conn: &turso::Connection) -> Result<i64, Error> {
    Ok(
        query("SELECT revision FROM stats_runtime_revision WHERE id=1")
            .fetch_one(conn)
            .await?
            .try_get(0)?,
    )
}
async fn message_keys(
    conn: &turso::Connection,
    chat: i64,
    start: Option<i64>,
    end: Option<i64>,
    cursor: Option<(i64, i64)>,
) -> Result<Vec<Row>, Error> {
    // Bound candidate keys before sparse SQL filters, so empty matches cannot lock-scan the entire chat.
    let upper = end.map(|d| d - 1).unwrap_or(i64::MAX);
    let mut keys = if let Some((date, id)) = cursor {
        query("SELECT date,id,from_user FROM stats_messages WHERE chat_id=? AND date=? AND id>? ORDER BY id LIMIT ?").bind(chat).bind(date).bind(id).bind(BATCH).fetch_all(conn).await?
    } else {
        Vec::new()
    };
    if keys.len() < BATCH as usize {
        let lower = cursor
            .map(|(d, _)| d.checked_add(1).ok_or(Error::Database))
            .transpose()?
            .unwrap_or(start.unwrap_or(i64::MIN));
        keys.extend(query("SELECT date,id,from_user FROM stats_messages WHERE chat_id=? AND date>=? AND date<=? ORDER BY date,id LIMIT ?").bind(chat).bind(lower).bind(upper).bind(BATCH-keys.len() as i64).fetch_all(conn).await?);
    }
    Ok(keys)
}

pub(crate) async fn aggregate(
    stats: &Stats,
    actor: i64,
    access: MemberAccess,
    endpoint: &str,
    p: &Params,
    limit: usize,
) -> Result<Value, Error> {
    if !matches!(
        endpoint,
        "overview"
            | "top-users"
            | "ecdf"
            | "message-types"
            | "activity-by-hour"
            | "activity-by-day"
            | "activity-by-week"
            | "history"
            | "daily-activity"
            | "activity-by-month"
            | "media-trend"
            | "user-stats"
            | "avg-message-length"
            | "title-history"
            | "rank"
            | "streak"
            | "corr"
            | "random"
            | "words"
            | "mention-stats"
            | "response-time"
            | "members"
            | "command-words"
            | "command-random"
            | "command-mentions"
    ) {
        return Err(Error::Invalid);
    }
    let now = Utc::now();
    let since = p
        .days
        .map(|d| (now - chrono::Duration::days(d)).timestamp_micros());
    let mut start = p
        .start
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_micros());
    if let Some(s) = since {
        start = Some(start.map_or(s, |a| a.max(s)));
    }
    let mut end = p
        .end
        .and_then(|d| d.succ_opt())
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_micros());
    if endpoint == "activity-by-month" {
        let s = if let Some(y) = p.year {
            NaiveDate::from_ymd_opt(y, 1, 1)
                .ok_or(Error::Invalid)?
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_micros()
        } else {
            (now - chrono::Duration::days(365)).timestamp_micros()
        };
        start = Some(start.map_or(s, |a| a.max(s)));
        if let Some(y) = p.year {
            let e = NaiveDate::from_ymd_opt(y + 1, 1, 1)
                .ok_or(Error::Invalid)?
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_micros();
            end = Some(end.map_or(e, |a| a.min(e)));
        }
    }
    let source_user = if matches!(endpoint, "rank" | "corr" | "command-mentions") {
        None
    } else {
        p.user_id
    };
    let terms = p.q.as_ref().map(|q| {
        yoink_stats_search::tokenize(q)
            .into_iter()
            .collect::<BTreeSet<_>>()
    });
    let (fence, mut seed, target_profile) = {
        let mut conn = stats.0.db.connection().await?;
        stats
            .authorize_on(
                &mut conn,
                actor,
                p.chat_id,
                false,
                endpoint == "members",
                access,
            )
            .await?;
        let fence = revision(&conn).await?;
        let seed = query("SELECT random()")
            .fetch_one(&*conn)
            .await?
            .try_get::<i64, _>(0)? as u64;
        let target = if endpoint == "command-mentions"
            && let Some(u) = p.user_id
        {
            profile(&*conn, u).await?
        } else {
            Value::Null
        };
        (fence, seed, target)
    };
    let incoming_pattern = target_profile["username"]
        .as_str()
        .map(|u| format!("@{}", u.to_lowercase()));
    let mut total = 0;
    let mut first = None;
    let mut last = None;
    let mut users = BTreeMap::<i64, User>::new();
    let mut kinds = BTreeMap::<String, usize>::new();
    let mut buckets = BTreeMap::<String, Bucket>::new();
    let mut words = BTreeMap::<String, usize>::new();
    let mut inbound = BTreeMap::<i64, usize>::new();
    let mut titles = Vec::<(i64, i64, String, Option<i64>, Option<String>)>::new();
    let mut days = BTreeSet::<NaiveDate>::new();
    let mut daily = BTreeMap::<NaiveDate, (f64, f64)>::new();
    let mut delays = BTreeMap::<i64, Delays>::new();
    let mut overall = Delays::default();
    let mut quote = Value::Null;
    let mut quotes = 0_u64;
    let stop = if endpoint == "command-words" {
        COMMAND_STOP
    } else {
        HTTP_STOP
    }
    .split_whitespace()
    .collect::<BTreeSet<_>>();
    let mut member_ids = BTreeSet::new();
    let mut cursor = None;
    loop {
        let (rows, finished) = {
            let mut conn = stats.0.db.connection().await?;
            stats
                .authorize_on(
                    &mut conn,
                    actor,
                    p.chat_id,
                    false,
                    endpoint == "members",
                    access,
                )
                .await?;
            if revision(&conn).await? != fence {
                return Err(Error::Unavailable);
            }
            let keys = message_keys(&conn, p.chat_id, start, end, cursor).await?;
            if endpoint == "members" {
                for key in &keys {
                    if let Some(u) = key.try_get::<Option<i64>, _>("from_user")? {
                        member_ids.insert(u);
                    }
                }
            }
            let finished = keys.len() < BATCH as usize;
            let Some(last) = keys.last() else {
                break;
            };
            cursor = Some((last.try_get("date")?, last.try_get("id")?));
            let bodies = terms.is_some()
                || matches!(
                    endpoint,
                    "words"
                        | "command-words"
                        | "mention-stats"
                        | "command-mentions"
                        | "random"
                        | "command-random"
                        | "avg-message-length"
                );
            let body_columns = if bodies {
                "m.text,m.caption"
            } else {
                "NULL AS text,NULL AS caption"
            };
            let title_column = if endpoint == "title-history" {
                "m.new_chat_title,coalesce(n.username,u.username) AS title_username"
            } else {
                "NULL AS new_chat_title,NULL AS title_username"
            };
            let reply_columns = if endpoint == "response-time" {
                "o.date AS original_date,o.from_user AS original_user"
            } else {
                "NULL AS original_date,NULL AS original_user"
            };
            let join = if endpoint == "response-time" {
                "LEFT JOIN stats_messages o ON o.chat_id=m.chat_id AND o.message_id=m.reply_to_message"
            } else if endpoint == "title-history" {
                "LEFT JOIN stats_user_latest_name n ON n.user_id=m.from_user LEFT JOIN users u ON u.id=m.from_user"
            } else {
                ""
            };
            let placeholders = vec!["?"; keys.len()].join(",");
            let sql = format!(
                "SELECT m.message_id,m.date,m.from_user,m.msg_type,{body_columns},{title_column},{reply_columns} FROM stats_messages m {join} WHERE m.id IN ({placeholders}) AND (? IS NULL OR m.from_user=?) AND (? IS NULL OR m.msg_type=?) ORDER BY m.date,m.id"
            );
            let mut request = query(&sql);
            for key in keys {
                request = request.bind(key.try_get::<i64, _>("id")?);
            }
            (
                request
                    .bind(source_user)
                    .bind(source_user)
                    .bind(p.msg_type.clone())
                    .bind(p.msg_type.clone())
                    .fetch_all(&*conn)
                    .await?,
                finished,
            )
        };
        for r in rows {
            let text = r.try_get::<Option<String>, _>("text")?;
            let caption = r.try_get::<Option<String>, _>("caption")?;
            if let Some(terms) = &terms {
                let body = format!(
                    "{} {}",
                    text.as_deref().unwrap_or(""),
                    caption.as_deref().unwrap_or("")
                );
                let tokens = yoink_stats_search::tokenize(&body)
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                if terms.is_empty() || !terms.is_subset(&tokens) {
                    continue;
                }
            }
            let timestamp = r.try_get::<i64, _>("date")?;
            let d = date(timestamp)?;
            let uid = r.try_get::<Option<i64>, _>("from_user")?;
            let kind = r.try_get::<String, _>("msg_type")?;
            total += 1;
            first = Some(first.map_or(timestamp, |a: i64| a.min(timestamp)));
            last = Some(last.map_or(timestamp, |a: i64| a.max(timestamp)));
            *kinds.entry(kind.clone()).or_default() += 1;
            if let Some(u) = uid {
                let state = users.entry(u).or_default();
                state.count += 1;
                state.first = Some(state.first.map_or(timestamp, |a| a.min(timestamp)));
                state.last = Some(state.last.map_or(timestamp, |a| a.max(timestamp)));
                if let Some(t) = text.as_ref().or(caption.as_ref()) {
                    let n = t.chars().count();
                    state.length_count += 1;
                    state.length_sum += n;
                    state.length_max = state.length_max.max(n);
                }
            }
            let key = match endpoint {
                "activity-by-hour" => Some(format!("{:02}", d.hour())),
                "activity-by-day" => Some(d.weekday().num_days_from_monday().to_string()),
                "activity-by-week" => Some(format!(
                    "{}:{:02}",
                    d.weekday().num_days_from_monday(),
                    d.hour()
                )),
                "activity-by-month" | "media-trend" => Some(d.format("%Y-%m").to_string()),
                "history" | "daily-activity" => Some(d.format("%Y-%m-%d").to_string()),
                _ => None,
            };
            if let Some(key) = key
                && (endpoint != "media-trend" || uid.is_some())
            {
                let b = buckets.entry(key).or_default();
                b.count += 1;
                b.text += usize::from(kind == "text");
                if let Some(u) = uid {
                    b.users.insert(u);
                }
            }
            if matches!(endpoint, "words" | "command-words") {
                for body in [text.as_deref(), caption.as_deref()].into_iter().flatten() {
                    for token in body.split_whitespace() {
                        let word = token
                            .chars()
                            .filter(|c| c.is_alphabetic())
                            .flat_map(char::to_lowercase)
                            .collect::<String>();
                        if word.chars().count() >= 3 && !stop.contains(word.as_str()) {
                            *words.entry(word).or_default() += 1;
                        }
                    }
                }
            }
            if matches!(endpoint, "mention-stats" | "command-mentions") {
                let body = text.as_deref().unwrap_or("");
                if endpoint != "command-mentions" || p.user_id.is_none() || uid == p.user_id {
                    for mention in mentions(body) {
                        *words.entry(format!("@{mention}")).or_default() += 1;
                    }
                }
                if endpoint == "command-mentions"
                    && let (Some(pattern), Some(u)) = (&incoming_pattern, uid)
                    && Some(u) != p.user_id
                    && body.to_lowercase().contains(pattern)
                {
                    *inbound.entry(u).or_default() += 1;
                }
            }
            if endpoint == "title-history"
                && let Some(title) = r.try_get::<Option<String>, _>("new_chat_title")?
            {
                titles.push((
                    timestamp,
                    r.try_get("message_id")?,
                    title,
                    uid,
                    r.try_get("title_username")?,
                ));
            }
            if endpoint == "streak" {
                days.insert(d.date_naive());
            }
            if endpoint == "corr"
                && let Some(u) = uid
                && p.target_user_id
                    .is_none_or(|target| u == target || Some(u) == p.user_id)
            {
                let pair = daily.entry(d.date_naive()).or_default();
                if Some(u) == p.user_id {
                    pair.0 += 1.0;
                } else {
                    pair.1 += 1.0;
                }
            }
            if endpoint == "response-time"
                && let (Some(u), Some(original)) =
                    (uid, r.try_get::<Option<i64>, _>("original_date")?)
            {
                let original_user = r.try_get::<Option<i64>, _>("original_user")?;
                let delay = timestamp - original;
                if Some(u) != original_user
                    && (1_000_000..=86_400_000_000).contains(&delay)
                    && p.target_user_id
                        .is_none_or(|target| original_user == Some(target))
                {
                    overall.add(delay);
                    if original_user.is_some() {
                        delays.entry(u).or_default().add(delay);
                    }
                }
            }
            if matches!(endpoint, "random" | "command-random")
                && let Some(t) = text
                && !t.is_empty()
                && (endpoint != "command-random" || (kind == "text" && t.chars().count() > 5))
            {
                quotes += 1;
                if random_below(&mut seed, quotes) == 0 {
                    quote = json!({"message_id":r.try_get::<i64,_>("message_id")?,"date":iso(Some(timestamp))?,"user_id":uid,"text":t});
                }
            }
        }
        if finished {
            break;
        }
        tokio::task::yield_now().await;
    }
    let reduced = match endpoint {
        "words" | "command-words" | "mention-stats" => Some(word_output(
            &words,
            limit,
            matches!(endpoint, "words" | "command-words"),
        )),
        "message-types" => Some(json!(
            ranked(&kinds, usize::MAX)
                .into_iter()
                .map(|(k, n)| json!({"type":k,"count":n}))
                .collect::<Vec<_>>()
        )),
        "random" | "command-random" => Some(quote),
        "title-history" => {
            titles.sort_by_key(|r| (r.0, r.1));
            let mut out = Vec::new();
            for (d, _, t, u, username) in titles {
                out.push(json!({"date":iso(Some(d))?,"title":t,"changed_by_user_id":u,"changed_by_username":username}));
            }
            Some(json!(out))
        }
        "corr" => Some({
            let u = p.user_id.ok_or(Error::Invalid)?;
            let n = daily.len() as f64;
            let x = daily.values().map(|r| r.0).sum::<f64>() / n;
            let y = daily.values().map(|r| r.1).sum::<f64>() / n;
            let covariance = daily.values().map(|r| (r.0 - x) * (r.1 - y)).sum::<f64>();
            let variance = (daily.values().map(|r| (r.0 - x).powi(2)).sum::<f64>()
                * daily.values().map(|r| (r.1 - y).powi(2)).sum::<f64>())
            .sqrt();
            json!({"user_id":u,"target_user_id":p.target_user_id,"correlation":if n>=2.0&&variance>0.0{Some(covariance/variance)}else{None}})
        }),
        "activity-by-hour" | "activity-by-day" | "activity-by-week" | "activity-by-month"
        | "media-trend" | "history" | "daily-activity" => Some(bucket_output(endpoint, buckets)?),
        _ => None,
    };
    let counts = if matches!(endpoint, "top-users" | "ecdf" | "rank") {
        users
            .iter()
            .map(|(uid, state)| (*uid, state.count))
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };
    let mut requested_profiles = BTreeSet::new();
    let mut deferred_profile = |u| {
        requested_profiles.insert(u);
        json!({"user_id":u})
    };
    let mut result = if let Some(result) = reduced {
        result
    } else if endpoint == "members" {
        Value::Null
    } else {
        match endpoint {
            "overview" => {
                let rc = 0;
                json!({"chat_id":p.chat_id,"total_messages":total,"unique_users":users.len(),"total_reactions":rc,"first_date":iso(first)?,"last_date":iso(last)?,"avg_per_day":average(total,first,last)})
            }
            "top-users" | "ecdf" => {
                let top = ranked(&counts, limit);
                let n = top.iter().map(|(_, n)| n).sum::<usize>();
                let mut cumulative = 0;
                let mut out = Vec::new();
                for (u, c) in top {
                    let mut v = with(deferred_profile(u), json!({"count":c}));
                    cumulative += c;
                    if endpoint == "ecdf" {
                        v["cumulative_pct"] =
                            json!(rounded(cumulative as f64 / n as f64 * 100.0, 1));
                    }
                    out.push(v);
                }
                json!(out)
            }
            "command-mentions" => {
                if p.user_id.is_none() {
                    word_output(&words, limit, false)
                } else {
                    let mut incoming = Vec::new();
                    for (u, n) in ranked(&inbound, limit) {
                        incoming.push(with(deferred_profile(u), json!({"count":n})));
                    }
                    json!({"profile":target_profile,"outgoing":word_output(&words,limit,false),"incoming":incoming})
                }
            }
            "user-stats" => {
                let u = p.user_id.ok_or(Error::Invalid)?;
                let rc = 0;
                with(
                    if total == 0 {
                        json!({"user_id":u,"username":null,"display_name":null})
                    } else {
                        deferred_profile(u)
                    },
                    json!({"total":total,"reaction_count":rc,"first_date":iso(first)?,"last_date":iso(last)?,"avg_per_day":average(total,first,last),"top_type":ranked(&kinds,1).first().map(|(k,_)|k)}),
                )
            }
            "rank" => {
                let u = p.user_id.ok_or(Error::Invalid)?;
                let c = counts.get(&u).copied().unwrap_or(0);
                let n = counts.values().sum::<usize>();
                with(
                    deferred_profile(u),
                    json!({"rank":if c==0{None}else{Some(1+counts.values().filter(|n|**n>c).count())},"count":c,"total_users":counts.len(),"grand_total":n,"percentage":if n==0{0.0}else{rounded(c as f64/n as f64*100.0,1)}}),
                )
            }
            "avg-message-length" => {
                let counts = users
                    .iter()
                    .filter(|(_, s)| s.length_count > 0)
                    .map(|(u, s)| (*u, s.length_count))
                    .collect();
                let mut out = Vec::new();
                for (u, n) in ranked(&counts, limit) {
                    let s = &users[&u];
                    out.push(with(deferred_profile(u),json!({"total":n,"avg_len":(s.length_sum as f64/n as f64).round() as usize,"max_len":s.length_max})));
                }
                json!(out)
            }
            "response-time" => {
                let (avg, median) = overall.formatted();
                let counts = delays.iter().map(|(u, s)| (*u, s.count)).collect();
                let mut out = Vec::new();
                for (u, n) in ranked(&counts, limit) {
                    let (a, m) = delays[&u].formatted();
                    out.push(with(
                        deferred_profile(u),
                        json!({"reply_count":n,"avg":a,"median":m}),
                    ));
                }
                json!({"overall_avg":avg,"overall_median":median,"total_replies":overall.count,"users":out})
            }
            "streak" => {
                let mut runs = Vec::<(usize, NaiveDate, NaiveDate)>::new();
                for d in days {
                    if let Some(r) = runs.last_mut()
                        && r.2.succ_opt() == Some(d)
                    {
                        r.0 += 1;
                        r.2 = d;
                    } else {
                        runs.push((1, d, d));
                    }
                }
                let current = runs.last().copied();
                let longest = runs.iter().max_by_key(|r| (r.0, r.2));
                with(
                    deferred_profile(p.user_id.ok_or(Error::Invalid)?),
                    json!({"current":current.map(|r|r.0).unwrap_or(0),"current_start":current.map(|r|r.1),"current_end":current.map(|r|r.2),"longest":longest.map(|r|r.0).unwrap_or(0),"longest_start":longest.map(|r|r.1),"longest_end":longest.map(|r|r.2)}),
                )
            }
            _ => return Err(Error::Invalid),
        }
    };

    let mut conn = stats.0.db.connection().await?;
    stats
        .authorize_on(
            &mut conn,
            actor,
            p.chat_id,
            false,
            endpoint == "members",
            access,
        )
        .await?;
    if revision(&conn).await? != fence {
        return Err(Error::Unavailable);
    }
    if endpoint == "members" {
        drop(conn);
        return members(
            stats, actor, access, fence, p, &users, member_ids, since, now,
        )
        .await;
    }
    let reaction_count = if matches!(endpoint, "overview" | "user-stats") {
        Some(if endpoint == "overview" {
            query("SELECT count(*) FROM stats_reactions WHERE chat_id=? AND (? IS NULL OR date>=?)")
                .bind(p.chat_id)
                .bind(since)
                .bind(since)
                .fetch_one(&*conn)
                .await?
                .try_get::<i64, _>(0)?
        } else {
            query("SELECT count(*) FROM stats_reactions WHERE chat_id=? AND user_id=? AND (? IS NULL OR date>=?)").bind(p.chat_id).bind(p.user_id.ok_or(Error::Invalid)?).bind(since).bind(since).fetch_one(&*conn).await?.try_get::<i64,_>(0)?
        })
    } else {
        None
    };
    let mut profiles = BTreeMap::new();
    for u in requested_profiles {
        profiles.insert(u, profile(&*conn, u).await?);
    }
    drop(conn);
    if let Some(rc) = reaction_count {
        result[if endpoint == "overview" {
            "total_reactions"
        } else {
            "reaction_count"
        }] = json!(rc);
    }
    if profiles.is_empty() {
        return Ok(result);
    }
    let rows = if result.is_array() {
        result.as_array_mut()
    } else if endpoint == "response-time" {
        result["users"].as_array_mut()
    } else if endpoint == "command-mentions" {
        result["incoming"].as_array_mut()
    } else {
        None
    };
    if let Some(rows) = rows {
        for row in rows {
            attach_profile(row, &profiles);
        }
    } else {
        attach_profile(&mut result, &profiles);
    }
    Ok(result)
}

fn attach_profile(value: &mut Value, profiles: &BTreeMap<i64, Value>) {
    if let Some(uid) = value["user_id"].as_i64()
        && let Some(profile) = profiles.get(&uid)
    {
        *value = with(profile.clone(), std::mem::take(value));
    }
}

fn word_output(words: &BTreeMap<String, usize>, limit: usize, is_word: bool) -> Value {
    json!(
        ranked(words, limit)
            .into_iter()
            .map(|(w, n)| if is_word {
                json!({"word":w,"count":n})
            } else {
                json!({"mention":w,"count":n})
            })
            .collect::<Vec<_>>()
    )
}
fn bucket_output(endpoint: &str, mut buckets: BTreeMap<String, Bucket>) -> Result<Value, Error> {
    if endpoint == "activity-by-hour" {
        for h in 0..24 {
            buckets.entry(format!("{h:02}")).or_default();
        }
    }
    if endpoint == "activity-by-day" {
        for d in 0..7 {
            buckets.entry(d.to_string()).or_default();
        }
    }
    let mut out = Vec::new();
    for (k, b) in buckets {
        out.push(match endpoint{
        "activity-by-hour"=>json!({"hour":k.parse::<u32>().map_err(|_|Error::Database)?,"count":b.count}),
        "activity-by-day"=>{let d=k.parse::<usize>().map_err(|_|Error::Database)?;json!({"day":d,"day_name":(["Mon","Tue","Wed","Thu","Fri","Sat","Sun"][d]),"count":b.count})},
        "activity-by-week"=>{let (d,h)=k.split_once(':').ok_or(Error::Database)?;json!({"day":d.parse::<u32>().map_err(|_|Error::Database)?,"hour":h.parse::<u32>().map_err(|_|Error::Database)?,"count":b.count})},
        "activity-by-month"=>json!({"month":k,"count":b.count}),
        "history"=>json!({"date":k,"count":b.count}),
        "daily-activity"=>json!({"date":k,"messages":b.count,"dau":b.users.len()}),
        "media-trend"=>json!({"month":k,"text":b.text,"media":b.count-b.text,"total":b.count,"media_pct":rounded((b.count-b.text) as f64/b.count as f64*100.0,1)}),
        _=>return Err(Error::Invalid),
    });
    }
    Ok(json!(out))
}

#[expect(clippy::too_many_arguments)]
async fn members(
    stats: &Stats,
    actor: i64,
    access: MemberAccess,
    fence: i64,
    p: &Params,
    users: &BTreeMap<i64, User>,
    mut ids: BTreeSet<i64>,
    since: Option<i64>,
    now: DateTime<Utc>,
) -> Result<Value, Error> {
    let mut message_cursor = None;
    loop {
        let keys = {
            let mut conn = stats.0.db.connection().await?;
            stats
                .authorize_on(&mut conn, actor, p.chat_id, false, true, access)
                .await?;
            if revision(&conn).await? != fence {
                return Err(Error::Unavailable);
            }
            message_keys(&conn, p.chat_id, None, None, message_cursor).await?
        };
        let done = keys.len() < BATCH as usize;
        for key in keys {
            message_cursor = Some((key.try_get("date")?, key.try_get("id")?));
            if let Some(u) = key.try_get::<Option<i64>, _>("from_user")? {
                ids.insert(u);
            }
        }
        if done {
            break;
        }
        tokio::task::yield_now().await;
    }
    let mut cursor = None;
    loop {
        let rows = {
            let mut conn = stats.0.db.connection().await?;
            stats
                .authorize_on(&mut conn, actor, p.chat_id, false, true, access)
                .await?;
            if revision(&conn).await? != fence {
                return Err(Error::Unavailable);
            }
            let op = if cursor.is_some() { ">" } else { ">=" };
            let sql = format!(
                "SELECT user_id FROM stats_group_members WHERE chat_id=? AND user_id{op}? ORDER BY user_id LIMIT ?"
            );
            query(&sql)
                .bind(p.chat_id)
                .bind(cursor.unwrap_or(i64::MIN))
                .bind(BATCH)
                .fetch_all(&*conn)
                .await?
        };
        let done = rows.len() < BATCH as usize;
        for row in rows {
            let u = row.try_get::<i64, _>(0)?;
            cursor = Some(u);
            ids.insert(u);
        }
        if done {
            break;
        }
        tokio::task::yield_now().await;
    }
    let cutoff = since.unwrap_or((now - chrono::Duration::days(90)).timestamp_micros());
    let mut out = Vec::new();
    let mut ids = ids.into_iter().peekable();
    while ids.peek().is_some() {
        let batch = {
            let mut conn = stats.0.db.connection().await?;
            stats
                .authorize_on(&mut conn, actor, p.chat_id, false, true, access)
                .await?;
            if revision(&conn).await? != fence {
                return Err(Error::Unavailable);
            }
            let mut batch = Vec::new();
            for u in ids.by_ref().take(BATCH as usize) {
                let state = users.get(&u);
                let reaction=query("SELECT count(*),max(date) FROM stats_reactions WHERE chat_id=? AND user_id=? AND (? IS NULL OR date>=?)").bind(p.chat_id).bind(u).bind(since).bind(since).fetch_one(&*conn).await?;
                let last = state
                    .and_then(|s| s.last)
                    .into_iter()
                    .chain(reaction.try_get::<Option<i64>, _>(1)?)
                    .max()
                    .filter(|d| *d > 0);
                let member=query("SELECT in_chat,synced_at FROM stats_group_members WHERE chat_id=? AND user_id=?").bind(p.chat_id).bind(u).fetch_optional(&*conn).await?;
                batch.push(with(profile(&*conn,u).await?,json!({"message_count":state.map(|s|s.count).unwrap_or(0),"reaction_count":reaction.try_get::<i64,_>(0)?,"first_seen_at":iso(state.and_then(|s|s.first))?,"last_active_at":iso(last)?,"is_active":last.is_some_and(|d|d>cutoff),"in_chat":member.as_ref().map(|r|r.try_get::<bool,_>(0)).transpose()?,"synced_at":iso(member.map(|r|r.try_get::<i64,_>(1)).transpose()?)?})));
            }
            batch
        };
        out.extend(batch);
        out.sort_by(|a, b| {
            b["last_active_at"]
                .as_str()
                .cmp(&a["last_active_at"].as_str())
                .then(a["user_id"].as_i64().cmp(&b["user_id"].as_i64()))
        });
        out.truncate(1000);
        tokio::task::yield_now().await;
    }
    let mut conn = stats.0.db.connection().await?;
    stats
        .authorize_on(&mut conn, actor, p.chat_id, false, true, access)
        .await?;
    if revision(&conn).await? != fence {
        return Err(Error::Unavailable);
    }
    Ok(json!(out))
}

#[cfg(test)]
mod tests {
    use super::Delays;
    #[test]
    fn second_bins_preserve_formatted_continuous_medians_and_rounded_averages() {
        for (values, expected) in [
            (vec![59_000_000, 60_000_000, 60_000_000], ("1m", "1m")),
            (vec![53_900_000, 66_100_000], ("1m", "1m")),
            (vec![1_100_000, 1_900_000, 1_800_000], ("2s", "1s")),
        ] {
            let mut delays = Delays::default();
            for value in values {
                delays.add(value);
            }
            assert_eq!(delays.formatted(), (expected.0.into(), expected.1.into()));
        }
    }
}
