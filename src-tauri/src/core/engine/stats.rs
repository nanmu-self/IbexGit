//! 提交统计（stats 对话框）：`git log` 原始点解析 + 分桶聚合。
//!
//! 纯函数、无 IO：解析（[`parse_stats_log`]）只认文本；聚合
//! （[`aggregate_stats`]）的 `now` 由调用方注入——生产传 `Local::now()`，
//! 测试传固定 `FixedOffset`。月/周/日的"本地"语义由 `now` 的时区决定，
//! 一次 log 产出四套桶（全历史按月、本月按天、本周按天、本日按小时），
//! 前端切 tab 零 IPC。

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

/// `git log` 输出的一个提交点：统计所需的最小集（作者 + committer 时间）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawStatCommit {
    pub name: String,
    pub email: String,
    /// Committer 时间（unix 秒，`%ct`）——cherry-pick/rebase 后反映落库时间。
    pub ts: i64,
}

/// 解析 `--format=%x1e%an%x1f%ae%x1f%ct` 的输出：记录以 \x1e 开头，
/// 字段以 \x1f 分隔（与 `parse_log_numstat` 同族的分隔约定）；
/// git 在每条记录末尾补 \n，时间戳字段需 trim 后再解析。
/// 时间戳非数字的残缺记录整条跳过。
pub fn parse_stats_log(raw: &str) -> Vec<RawStatCommit> {
    let mut out = Vec::new();
    for record in raw.split('\x1e') {
        if record.is_empty() {
            continue;
        }
        let mut fields = record.split('\x1f');
        let name = fields.next().unwrap_or("").to_string();
        let email = fields.next().unwrap_or("").to_string();
        let Some(ts) = fields.next().and_then(|t| t.trim().parse::<i64>().ok()) else {
            continue; // truncated tail record
        };
        out.push(RawStatCommit { name, email, ts });
    }
    out
}

/// 一位贡献者（按 email 聚合；显示名取最新一次提交所用名字）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ContributorStat {
    pub name: String,
    pub email: String,
    pub count: u32,
}

/// 一个时间桶。`key` 是规范键（月 `2026-08`、日 `2026-08-05`、小时 `14`，
/// 均为零填充定宽、本地时区），显示格式由前端负责。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct StatBucket {
    pub key: String,
    pub count: u32,
}

/// 一个时间周期的合计：贡献者表（按提交数降序）+ 总数。
/// 总览用全期；本月/本周/本日的口径与对应桶轴完全一致。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PeriodStats {
    pub total: u32,
    pub contributors: Vec<ContributorStat>,
}

/// 一次 `commit_stats` 命令的全部结果：一次 log，四个周期 + 四套桶。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct CommitStatsDto {
    /// 全期（总览）。
    pub all: PeriodStats,
    /// 本月（1 号 → 今天，含未来小时偏差之外的当月提交）。
    pub month: PeriodStats,
    /// 本周（ISO 周一 → 今天）。
    pub week: PeriodStats,
    /// 本日（00 点 → 当前小时）。
    pub today: PeriodStats,
    /// 全历史按月：从首个提交月到当前月，含中间空月（横轴可滚动）。
    pub months: Vec<StatBucket>,
    /// 本月按天：1 号到今天。
    pub month_days: Vec<StatBucket>,
    /// 本周按天：ISO 周一到今天。
    pub week_days: Vec<StatBucket>,
    /// 本日按小时：00 点到当前小时。
    pub today_hours: Vec<StatBucket>,
}

fn day_key(d: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

fn month_key(d: NaiveDate) -> String {
    format!("{:04}-{:02}", d.year(), d.month())
}

/// 把解析出的原始点聚合成四套桶 + 贡献者表。
///
/// 输入顺序沿用 `git log`（新→旧）：贡献者显示名取每个 email 的首次
/// 出现（即最新）名字。桶一律零填充：月轴从首个提交月铺到当前月，
/// 日/小时轴只铺到 `now` 为止（未来的提交因时钟偏差存在，不进轴）。
pub fn aggregate_stats<Tz: TimeZone>(
    points: &[RawStatCommit],
    now: DateTime<Tz>,
) -> CommitStatsDto {
    // 空分支（无任何非 merge 提交）：周期与桶全空，前端直接走空态。
    if points.is_empty() {
        return CommitStatsDto {
            all: PeriodStats::default(),
            month: PeriodStats::default(),
            week: PeriodStats::default(),
            today: PeriodStats::default(),
            months: Vec::new(),
            month_days: Vec::new(),
            week_days: Vec::new(),
            today_hours: Vec::new(),
        };
    }

    let tz = now.timezone();
    let today = now.date_naive();
    let month_start = today.with_day(1).expect("day 1 is always valid");
    let week_start = today - Duration::days(now.weekday().num_days_from_monday() as i64);

    // 贡献者按 email 聚合（同一人改名并到一处；同名不同邮箱按 GitHub
    // 口径分开），四个周期各一套；周期口径与对应桶轴完全一致。
    let mut all = ContribAcc::new();
    let mut month = ContribAcc::new();
    let mut week = ContribAcc::new();
    let mut today_acc = ContribAcc::new();

    let mut months: HashMap<String, u32> = HashMap::new();
    let mut month_days: HashMap<String, u32> = HashMap::new();
    let mut week_days: HashMap<String, u32> = HashMap::new();
    let mut today_hours: HashMap<String, u32> = HashMap::new();
    let mut first_commit_day: Option<NaiveDate> = None;

    for p in points {
        let Some(dt) = tz.timestamp_opt(p.ts, 0).single() else {
            continue;
        };
        let d = dt.date_naive();
        first_commit_day = Some(first_commit_day.map_or(d, |min| min.min(d)));
        all.add(p);
        *months.entry(month_key(d)).or_insert(0) += 1;
        // 未来时间戳（时钟偏差）只进月轴/全期，不进"当前周期"。
        if d >= month_start && d <= today {
            month.add(p);
            *month_days.entry(day_key(d)).or_insert(0) += 1;
        }
        if d >= week_start && d <= today {
            week.add(p);
            *week_days.entry(day_key(d)).or_insert(0) += 1;
        }
        if d == today && dt.hour() <= now.hour() {
            today_acc.add(p);
            *today_hours.entry(format!("{:02}", dt.hour())).or_insert(0) += 1;
        }
    }

    // 月轴：首个提交月 → 当前月，逐月铺零。
    let mut month_buckets = Vec::new();
    if let Some(first) = first_commit_day {
        let mut cursor = first.with_day(1).expect("day 1 is always valid");
        let end = now.date_naive().with_day(1).expect("day 1 is always valid");
        while cursor <= end {
            month_buckets.push(StatBucket {
                key: month_key(cursor),
                count: months.get(&month_key(cursor)).copied().unwrap_or(0),
            });
            cursor = cursor
                .checked_add_months(Months::new(1))
                .expect("month cursor stays in range");
        }
    }

    // 日/小时轴：区间铺零 + 取数。
    let fill_days = |from: NaiveDate, map: &HashMap<String, u32>| -> Vec<StatBucket> {
        let mut out = Vec::new();
        let mut cursor = from;
        while cursor <= today {
            out.push(StatBucket {
                key: day_key(cursor),
                count: map.get(&day_key(cursor)).copied().unwrap_or(0),
            });
            cursor += Duration::days(1);
        }
        out
    };

    let mut today_hour_buckets = Vec::new();
    for h in 0..=now.hour() {
        let key = format!("{:02}", h);
        today_hour_buckets.push(StatBucket {
            count: today_hours.get(&key).copied().unwrap_or(0),
            key,
        });
    }

    CommitStatsDto {
        all: all.finish(),
        month: month.finish(),
        week: week.finish(),
        today: today_acc.finish(),
        months: month_buckets,
        month_days: fill_days(month_start, &month_days),
        week_days: fill_days(week_start, &week_days),
        today_hours: today_hour_buckets,
    }
}

/// 单个周期的贡献者累计器：按 email 计数，显示名取最新一次提交用的
/// 名字（git log 新→旧，首次出现即最新）。
struct ContribAcc<'a> {
    counts: HashMap<&'a str, u32>,
    names: HashMap<&'a str, &'a str>,
}

impl<'a> ContribAcc<'a> {
    fn new() -> Self {
        Self {
            counts: HashMap::new(),
            names: HashMap::new(),
        }
    }

    fn add(&mut self, p: &'a RawStatCommit) {
        *self.counts.entry(p.email.as_str()).or_insert(0) += 1;
        self.names
            .entry(p.email.as_str())
            .or_insert(p.name.as_str());
    }

    /// 按提交数降序（并列时按名字、email）。
    fn finish(self) -> PeriodStats {
        let mut contributors: Vec<ContributorStat> = self
            .counts
            .into_iter()
            .map(|(email, count)| ContributorStat {
                name: self.names[email].to_string(),
                email: email.to_string(),
                count,
            })
            .collect();
        contributors.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.email.cmp(&b.email))
        });
        PeriodStats {
            total: contributors.iter().map(|c| c.count).sum(),
            contributors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    /// UTC+8，和常见中文环境一致；`aggregate_stats` 对时区无假设，
    /// 生产走 `Local`，测试用固定偏移保证确定性。
    fn tz() -> FixedOffset {
        FixedOffset::east_opt(8 * 3600).unwrap()
    }

    fn pt(name: &str, email: &str, y: i32, m: u32, d: u32, h: u32) -> RawStatCommit {
        let dt = tz()
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .single()
            .expect("valid test timestamp");
        RawStatCommit {
            name: name.to_string(),
            email: email.to_string(),
            ts: dt.timestamp(),
        }
    }

    #[test]
    fn parse_two_records() {
        // 与真实输出一致：每条记录末尾有 \n。
        let raw = "\x1e张三\x1fz@x.com\x1f1725312000\n\x1e李四\x1fl@x.com\x1f1725225600\n";
        let out = parse_stats_log(raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "张三");
        assert_eq!(out[0].email, "z@x.com");
        assert_eq!(out[0].ts, 1725312000);
        assert_eq!(out[1].email, "l@x.com");
    }

    #[test]
    fn parse_skips_truncated_tail_and_leading_empty() {
        let raw = "\x1ea\x1fa@x.com\x1f100\x1eb\x1fb@x.com\x1fnot-a-number";
        assert_eq!(parse_stats_log(raw).len(), 1);
        assert_eq!(parse_stats_log("").len(), 0);
    }

    #[test]
    fn parse_tolerates_empty_author_fields() {
        let raw = "\x1e\x1f\x1f42";
        let out = parse_stats_log(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "");
        assert_eq!(out[0].email, "");
        assert_eq!(out[0].ts, 42);
    }

    #[test]
    fn aggregate_groups_by_email_keeping_newest_name() {
        // git log 新→旧：a@x 最早的提交叫 "Old A"，最新叫 "A One"。
        // 同名不同邮箱（A Two / a2@x）按 GitHub 口径分开。
        let points = vec![
            pt("A One", "a@x", 2026, 9, 3, 10),
            pt("A Two", "a2@x", 2026, 9, 2, 10),
            pt("Old A", "a@x", 2026, 8, 1, 10),
        ];
        let now = tz()
            .with_ymd_and_hms(2026, 9, 3, 12, 0, 0)
            .single()
            .unwrap();
        let dto = aggregate_stats(&points, now);
        assert_eq!(dto.all.total, 3);
        assert_eq!(dto.all.contributors.len(), 2);
        assert_eq!(dto.all.contributors[0].name, "A One");
        assert_eq!(dto.all.contributors[0].email, "a@x");
        assert_eq!(dto.all.contributors[0].count, 2);
        assert_eq!(dto.all.contributors[1].count, 1);
    }

    #[test]
    fn aggregate_fills_zero_buckets_all_axes() {
        // now = 2026-09-03（周四）14:30。提交散布在 7/8/9 月。
        let points = vec![
            pt("a", "a@x", 2026, 7, 15, 8),
            pt("a", "a@x", 2026, 8, 20, 9),
            // 本月 9-01（在本周外：周一是 8-31）
            pt("a", "a@x", 2026, 9, 1, 10),
            // 今天 14 点（小时上界）
            pt("a", "a@x", 2026, 9, 3, 14),
        ];
        let now = tz()
            .with_ymd_and_hms(2026, 9, 3, 14, 30, 0)
            .single()
            .unwrap();
        let dto = aggregate_stats(&points, now);

        // 月轴：2026-07 → 2026-09，零填充。
        assert_eq!(
            dto.months
                .iter()
                .map(|b| (b.key.as_str(), b.count))
                .collect::<Vec<_>>(),
            vec![("2026-07", 1), ("2026-08", 1), ("2026-09", 2)]
        );

        // 本月：1 号 → 今天（3 天），9-02 为零。
        assert_eq!(
            dto.month_days
                .iter()
                .map(|b| (b.key.as_str(), b.count))
                .collect::<Vec<_>>(),
            vec![("2026-09-01", 1), ("2026-09-02", 0), ("2026-09-03", 1),]
        );

        // 本周：周一 8-31 → 今天，8-31/9-01 为零、9-02 为零、9-03 为 1。
        assert_eq!(
            dto.week_days
                .iter()
                .map(|b| (b.key.as_str(), b.count))
                .collect::<Vec<_>>(),
            vec![
                ("2026-08-31", 0),
                ("2026-09-01", 1),
                ("2026-09-02", 0),
                ("2026-09-03", 1),
            ]
        );

        // 本日：00 → 当前小时（14），14 桶为 1。
        assert_eq!(dto.today_hours.len(), 15);
        assert_eq!(dto.today_hours[0].key, "00");
        assert_eq!(dto.today_hours[0].count, 0);
        assert_eq!(dto.today_hours[14].key, "14");
        assert_eq!(dto.today_hours[14].count, 1);

        // 周期口径与对应桶轴一致：全期 4，本月/本周 2，本日 1。
        assert_eq!(dto.all.total, 4);
        assert_eq!(dto.month.total, 2);
        assert_eq!(dto.week.total, 2);
        assert_eq!(dto.today.total, 1);
        assert_eq!(dto.today.contributors.len(), 1);
        assert_eq!(dto.today.contributors[0].email, "a@x");
    }

    #[test]
    fn aggregate_excludes_future_and_out_of_range() {
        // 未来时间戳（时钟偏差）只进月轴；上个月（周外）不进本周桶。
        let points = vec![
            pt("a", "a@x", 2026, 9, 3, 20), // 今天但晚于 now 14:30
            pt("a", "a@x", 2026, 8, 30, 8), // 周一 8-31 之前的周日
        ];
        let now = tz()
            .with_ymd_and_hms(2026, 9, 3, 14, 30, 0)
            .single()
            .unwrap();
        let dto = aggregate_stats(&points, now);

        assert_eq!(
            dto.months
                .iter()
                .map(|b| (b.key.as_str(), b.count))
                .collect::<Vec<_>>(),
            vec![("2026-08", 1), ("2026-09", 1)]
        );
        // 本月：只有 9-03 有提交，但小时桶里没有 20 点。
        assert_eq!(dto.month_days.last().unwrap().count, 1);
        assert!(dto.today_hours.iter().all(|b| b.count == 0));
        assert_eq!(dto.today_hours.last().unwrap().key, "14");
        // 周期口径：未来小时的提交按天计仍在本周/本月，但不在本日。
        assert_eq!(dto.all.total, 2);
        assert_eq!(dto.month.total, 1);
        assert_eq!(dto.week.total, 1);
        assert_eq!(dto.today.total, 0);
        // 本周从 8-31 起：8-30（周日）的提交不在本周；但 9-03 的
        // 未来小时提交按天计仍在本周/本月。
        assert_eq!(dto.week_days.last().unwrap().count, 1);
        assert!(dto.week_days[..dto.week_days.len() - 1]
            .iter()
            .all(|b| b.count == 0));
        // 月轴仍然从首个提交月（8 月）开始。
        assert_eq!(dto.months.first().unwrap().key, "2026-08");
    }

    #[test]
    fn aggregate_empty_points() {
        let now = tz()
            .with_ymd_and_hms(2026, 9, 3, 14, 30, 0)
            .single()
            .unwrap();
        let dto = aggregate_stats(&[], now);
        assert_eq!(dto.all.total, 0);
        assert!(dto.all.contributors.is_empty());
        assert_eq!(dto.month.total, 0);
        assert_eq!(dto.week.total, 0);
        assert_eq!(dto.today.total, 0);
        assert!(dto.months.is_empty());
        assert!(dto.month_days.is_empty());
        assert!(dto.week_days.is_empty());
        assert!(dto.today_hours.is_empty());
    }
}
