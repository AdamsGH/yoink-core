use crate::{Error, Insight};
use chrono::{Datelike, Utc};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use yoink_db::query;
impl Insight {
    pub async fn stats(&self, uid: i64) -> Result<Value, Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        let now = Utc::now();
        let today = now.date_naive();
        let week = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
        let month = today - chrono::Duration::days(30);
        let rows=query("SELECT strftime('%Y-%m-%d',created_at/1000000,'unixepoch') AS day,command,status,alias_key,route,CASE WHEN video_seconds IS NOT NULL THEN 'youtube' ELSE 'web' END AS kind,count(*) AS n,sum(CASE WHEN video_seconds IS NOT NULL THEN max(0,video_seconds/60.0) ELSE 0 END) AS vm,sum(CASE WHEN video_seconds IS NULL THEN max(0,coalesce(content_chars,0)/1000.0) ELSE 0 END) AS rm FROM insight_usage_log WHERE user_id=? AND status IN ('ok','cached') GROUP BY day,command,status,alias_key,route,kind").bind(uid).fetch_all(&*c).await?;
        let (mut total, mut tw, mut td, mut tt, mut ttw, mut ttd, mut yt, mut web, mut vm, mut rm) =
            (0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0f64, 0f64);
        let mut cmds = BTreeMap::<String, i64>::new();
        let mut days = BTreeMap::<String, i64>::new();
        let mut aliases = BTreeMap::<String, i64>::new();
        let mut tdays = BTreeMap::<String, (i64, f64, BTreeMap<String, i64>)>::new();
        let mut routes = BTreeMap::<String, i64>::new();
        for r in rows {
            let day: String = r.try_get("day")?;
            let date =
                chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").map_err(|_| Error::Database)?;
            let command: String = r.try_get("command")?;
            let status: String = r.try_get("status")?;
            let n: i64 = r.try_get("n")?;
            let tldr = command.starts_with("tldr");
            if status == "ok" {
                total += n;
                if date >= week {
                    tw += n
                }
                if date == today {
                    td += n
                }
                *cmds
                    .entry(if tldr { "tldr".into() } else { command })
                    .or_default() += n;
                if date >= month {
                    *days.entry(day.clone()).or_default() += n
                }
            }
            if tldr {
                tt += n;
                if date >= week {
                    ttw += n
                }
                if date == today {
                    ttd += n
                }
                let alias = r
                    .try_get::<Option<String>, _>("alias_key")?
                    .unwrap_or("_none".into());
                *aliases.entry(alias.clone()).or_default() += n;
                let kind: String = r.try_get("kind")?;
                if kind == "youtube" {
                    yt += n
                } else {
                    web += n
                }
                let v: f64 = r.try_get("vm")?;
                let w: f64 = r.try_get("rm")?;
                vm += v;
                rm += w;
                let route: String = r.try_get("route")?;
                *routes.entry(route).or_default() += n;
                if date >= month {
                    let entry = tdays.entry(day).or_default();
                    entry.0 += n;
                    entry.1 += v + w;
                    *entry.2.entry(alias).or_default() += n
                }
            }
        }
        fn round(v: f64) -> f64 {
            (v * 10.0).round() / 10.0
        }
        Ok(
            json!({"total_summaries":total,"this_week":tw,"today":td,"by_command":cmds,"by_day":days.into_iter().map(|(date,count)|json!({"date":date,"count":count})).collect::<Vec<_>>(),"tldr":{"total":tt,"this_week":ttw,"today":ttd,"by_alias":aliases.into_iter().map(|(alias,count)|json!({"alias":alias,"count":count})).collect::<Vec<_>>(),"by_kind":{"youtube":yt,"web":web},"by_day":tdays.into_iter().map(|(date,(count,minutes,aliases))|json!({"date":date,"count":count,"minutes_saved":round(minutes),"by_alias":aliases})).collect::<Vec<_>>(),"minutes_saved":round(vm+rm),"video_minutes_saved":round(vm),"reading_minutes_saved":round(rm),"by_route":routes}}),
        )
    }
}
