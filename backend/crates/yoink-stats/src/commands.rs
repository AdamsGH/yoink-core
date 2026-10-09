use crate::{Error, Stats, analytics::Params};
use serde_json::Value;
use yoink_db::query;
use yoink_i18n::Language;

/// Authorization provenance must survive private delivery and command replay.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CommandResult {
    pub chunks: Vec<String>,
    pub source_group: Option<i64>,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn label(key: &str, lang: Language) -> String {
    let labels = [
        ("user_id", "User ID", "ID пользователя"),
        ("username", "Username", "Имя пользователя"),
        ("display_name", "Name", "Имя"),
        ("count", "Messages", "Сообщения"),
        ("total", "Total", "Всего"),
        ("reaction_count", "Reactions", "Реакции"),
        ("first_date", "First message", "Первое сообщение"),
        ("last_date", "Last message", "Последнее сообщение"),
        ("avg_per_day", "Average per day", "В среднем за день"),
        ("top_type", "Top type", "Основной тип"),
        ("type", "Type", "Тип"),
        ("hour", "Hour", "Час"),
        ("day", "Day", "День"),
        ("date", "Date", "Дата"),
        ("month", "Month", "Месяц"),
        ("word", "Word", "Слово"),
        ("mention", "Mention", "Упоминание"),
        ("title", "Title", "Название"),
        ("rank", "Rank", "Место"),
        ("total_users", "Users", "Пользователи"),
        ("percentage", "Percent", "Процент"),
        ("grand_total", "Group total", "Всего в группе"),
        ("current", "Current streak", "Текущая серия"),
        ("current_start", "Current start", "Начало серии"),
        ("current_end", "Current end", "Конец серии"),
        ("longest", "Longest streak", "Самая длинная серия"),
        ("longest_start", "Longest start", "Начало длинной серии"),
        ("longest_end", "Longest end", "Конец длинной серии"),
        ("correlation", "Correlation", "Корреляция"),
        ("target_user_id", "Target user", "Другой пользователь"),
        ("overall_avg", "Average reply time", "Среднее время ответа"),
        (
            "overall_median",
            "Median reply time",
            "Медиана времени ответа",
        ),
        ("total_replies", "Replies", "Ответы"),
        ("reply_count", "Replies", "Ответы"),
        ("avg", "Average", "Среднее"),
        ("median", "Median", "Медиана"),
        ("users", "Users", "Пользователи"),
        ("text", "Text", "Текст"),
        ("message_id", "Message ID", "ID сообщения"),
        (
            "cumulative_pct",
            "Cumulative percent",
            "Накопленный процент",
        ),
    ];
    labels
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, en, ru)| if lang == Language::Russian { *ru } else { *en })
        .map(str::to_owned)
        .unwrap_or_else(|| key.replace('_', " "))
}
fn plain(v: &Value, lang: Language) -> String {
    match v {
        Value::Null => if lang == Language::Russian {
            "Нет данных."
        } else {
            "No data."
        }
        .into(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, v)| format!("{}. {}", i + 1, plain(v, lang)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(o) => o
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "has_photo" | "day_name"))
            .map(|(k, v)| format!("{}: {}", label(k, lang), plain(v, lang)))
            .collect::<Vec<_>>()
            .join("\n"),
        v => v.to_string(),
    }
}
fn chunks(heading: &str, body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chunk = String::new();
    for c in body.chars() {
        let escaped = escape(&c.to_string());
        if chunk.len() + escaped.len() > 3400 {
            out.push(format!("<b>{heading}</b>\n<pre>{chunk}</pre>"));
            chunk.clear();
        }
        chunk.push_str(&escaped);
    }
    if !chunk.is_empty() {
        out.push(format!("<b>{heading}</b>\n<pre>{chunk}</pre>"));
    }
    out
}
pub fn help(language: Language) -> Vec<String> {
    let intro = if language == Language::Russian {
        "Запускай /stats в группе. В личке доступна только сводка user. Даты: YYYY-MM-DD."
    } else {
        "Run /stats in a group. In private, only user summaries are available. Dates: YYYY-MM-DD."
    };
    chunks(
        "/stats",
        &format!(
            "{intro}\ncounts [-n N] [--type TYPE]\nhours | days | week [--user ID]\nhistory [--days N] [--user ID]\nuser [ID]\ntypes [--user ID]\nwords [-n N] [-q QUERY]\nrandom [--user ID] [-q QUERY]\nrank ID\nstreak ID\necdf\ncorr ID [TARGET_ID]\ndelta ID [TARGET_ID]\ntitles\nmention [ID] [-n N]\n--start DATE --end DATE -q QUERY"
        ),
    )
}
impl Stats {
    /// Arguments are Telegram command tokens, excluding /stats. Results are independently valid HTML.
    pub async fn command(
        &self,
        user: i64,
        chat: i64,
        args: &[String],
        language: Language,
    ) -> Result<Vec<String>, Error> {
        Ok(self
            .command_result(user, chat, args, language)
            .await?
            .chunks)
    }
    pub async fn command_result(
        &self,
        user: i64,
        chat: i64,
        args: &[String],
        language: Language,
    ) -> Result<CommandResult, Error> {
        self.role(user).await?;
        let Some(command) = args.first().map(String::as_str) else {
            return Ok(CommandResult {
                chunks: help(language),
                source_group: None,
            });
        };
        if chat >= 0 && command != "user" {
            return Ok(CommandResult {
                chunks: help(language),
                source_group: None,
            });
        }
        let mut p = Params {
            chat_id: chat,
            ..Default::default()
        };
        let mut positional = Vec::new();
        let mut i = 1;
        while i < args.len() {
            let a = &args[i];
            if a.starts_with('-') {
                i += 1;
                let value = args.get(i).ok_or(Error::Invalid)?;
                match a.as_str() {
                    "-n" | "--limit" => p.limit = Some(value.parse().map_err(|_| Error::Invalid)?),
                    "--user" => p.user_id = Some(value.parse().map_err(|_| Error::Invalid)?),
                    "--days" => p.days = Some(value.parse().map_err(|_| Error::Invalid)?),
                    "--type" => p.msg_type = Some(value.clone()),
                    "--start" => p.start = Some(value.parse().map_err(|_| Error::Invalid)?),
                    "--end" => p.end = Some(value.parse().map_err(|_| Error::Invalid)?),
                    "-q" | "--lquery" => p.q = Some(value.clone()),
                    _ => return Err(Error::Invalid),
                }
            } else {
                positional.push(a.parse::<i64>().map_err(|_| Error::Invalid)?);
            }
            i += 1;
        }
        crate::analytics::validate_days(p.days)?;
        if positional.len() > 2 || positional.iter().any(|u| *u <= 0) {
            return Err(Error::Invalid);
        }
        if matches!(
            command,
            "user" | "rank" | "streak" | "corr" | "delta" | "mention"
        ) {
            if let Some(uid) = positional.first() {
                p.user_id = Some(*uid);
            } else if command == "user" {
                p.user_id = Some(user);
            } else if matches!(command, "rank" | "streak" | "corr" | "delta") {
                return Err(Error::Invalid);
            }
            if let Some(target) = positional.get(1) {
                if !matches!(command, "corr" | "delta") {
                    return Err(Error::Invalid);
                }
                p.target_user_id = Some(*target);
            }
        } else if !positional.is_empty() {
            return Err(Error::Invalid);
        }
        if command == "counts" && p.limit.is_none() {
            p.limit = Some(20);
        }
        if command == "history" && p.days.is_none() && p.start.is_none() {
            p.days = Some(30);
        }
        if command == "history"
            && p.start.is_none()
            && let Some(end) = p.end
        {
            let duration =
                chrono::Duration::try_days(p.days.unwrap_or(30)).ok_or(Error::Invalid)?;
            p.start = Some(end.checked_sub_signed(duration).ok_or(Error::Invalid)?);
            p.days = None;
        }
        if command == "history" && p.start.is_some() {
            p.days = None;
        }
        if chat >= 0 {
            let target = p.user_id.ok_or(Error::Invalid)?;
            let rows=query("SELECT DISTINCT chat_id FROM stats_messages WHERE from_user=? AND chat_id<0 ORDER BY chat_id").bind(target).fetch_all(&self.0.db).await?;
            let mut found = None;
            for row in rows {
                let group = row.try_get::<i64, _>(0)?;
                match self.authorize(user, group, false, false).await {
                    Ok(_) => {
                        found = Some(group);
                        break;
                    }
                    Err(Error::Forbidden) => (),
                    Err(e) => return Err(e),
                }
            }
            let Some(group) = found else {
                return Ok(CommandResult {
                    chunks: chunks(
                        "/stats user",
                        if language == Language::Russian {
                            "Нет данных."
                        } else {
                            "No data."
                        },
                    ),
                    source_group: None,
                });
            };
            p.chat_id = group;
        }
        let source_group = Some(p.chat_id);
        Ok(CommandResult {
            chunks: self
                .render_command_chunks(user, command, p, language)
                .await?,
            source_group,
        })
    }
    /// Perform external membership preflight without holding the journal transaction.
    pub async fn command_preflight(
        &self,
        user: i64,
        source_group: Option<i64>,
    ) -> Result<crate::MemberAccess, Error> {
        match source_group {
            Some(group) => self.authorize(user, group, false, false).await,
            None => {
                self.role(user).await?;
                Ok(crate::MemberAccess::default())
            }
        }
    }
    /// Caller owns BEGIN IMMEDIATE on the shared identity/journal database.
    /// Check local authority after connection wait, immediately before admission.
    pub async fn command_authority_on(
        &self,
        connection: &turso::Connection,
        user: i64,
        source_group: Option<i64>,
        access: crate::MemberAccess,
    ) -> Result<Value, Error> {
        use yoink_access::Role;
        let snapshot = self
            .0
            .identity
            .snapshot_in_transaction(connection, user, None)
            .await
            .map_err(|_| Error::Database)?
            .ok_or(Error::Forbidden)?;
        let role = snapshot.principal.role;
        if role < Role::User || snapshot.principal.is_blocked(snapshot.now) {
            return Err(Error::Forbidden);
        }
        let mut assigned = false;
        if let Some(group) = source_group {
            if group >= 0
                || query("SELECT 1 FROM groups WHERE id=? AND enabled=1")
                    .bind(group)
                    .fetch_optional(connection)
                    .await?
                    .is_none()
            {
                return Err(Error::Forbidden);
            }
            assigned = query("SELECT 1 FROM user_group_policies WHERE user_id=? AND group_id=?")
                .bind(user)
                .bind(group)
                .fetch_optional(connection)
                .await?
                .is_some();
            if role < Role::Admin
                && ((role == Role::Moderator && !assigned)
                    || (role == Role::User && !access.member))
            {
                return Err(Error::Forbidden);
            }
        }
        Ok(
            serde_json::json!({"source_group":source_group,"role":format!("{role:?}"),"assigned":assigned,"member":access.member}),
        )
    }
    pub async fn render_command_chunks(
        &self,
        user: i64,
        command: &str,
        p: Params,
        language: Language,
    ) -> Result<Vec<String>, Error> {
        let endpoint = match command {
            "counts" => "top-users",
            "hours" => "activity-by-hour",
            "days" => "activity-by-day",
            "week" => "activity-by-week",
            "history" => "history",
            "user" => "user-stats",
            "types" => "message-types",
            "words" => "command-words",
            "ecdf" => "ecdf",
            "titles" => "title-history",
            "mention" => "command-mentions",
            "delta" => "response-time",
            "random" => "command-random",
            "rank" | "streak" | "corr" => command,
            _ => return Err(Error::Invalid),
        };
        let result = self.analytics(user, endpoint, p).await?;
        let heading = format!(
            "{}: {}",
            if language == Language::Russian {
                "Статистика"
            } else {
                "Statistics"
            },
            escape(command)
        );
        let body = if result.as_array().is_some_and(Vec::is_empty) {
            plain(&Value::Null, language)
        } else {
            plain(&result, language)
        };
        Ok(chunks(&heading, &body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunks_are_complete_html_and_never_split_entities_or_unicode() {
        let out = chunks("stats", &"<&Привет>".repeat(2000));
        assert!(out.len() > 1);
        assert!(
            out.iter()
                .all(|s| s.len() < 4096 && s.ends_with("</pre>") && !s.contains("<&"))
        );
    }
    #[test]
    fn help_and_labels_cover_both_languages() {
        assert!(help(Language::Russian)[0].contains("группе"));
        assert!(help(Language::English)[0].contains("group"));
        assert_eq!(
            label("overall_median", Language::Russian),
            "Медиана времени ответа"
        );
    }
}
