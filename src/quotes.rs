//! 每日名言。
//!
//! 数据取自古文岛（原古诗文网）的「推荐名句」页面 <https://www.gushiwen.cn/mingjus/>。
//! 该站没有 JSON 接口，只有 HTML，因此按 `div.cont` 区块解析「名句 + 出处」。
//!
//! 抓取策略：
//! 1. 每天抓取一次，结果按日期缓存到配置文件同目录，保证同一天内显示稳定；
//! 2. 抓取或解析失败时沿用上一次的缓存；
//! 3. 连缓存也没有时使用内置的 30 条文案。

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Datelike;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

/// 古文岛「推荐名句」页面
const MINGJU_URL: &str = "https://www.gushiwen.cn/mingjus/";
/// 网络超时
const TIMEOUT: Duration = Duration::from_secs(8);
/// 缓存文件名（与配置文件同目录）
pub const CACHE_FILE: &str = "quote-cache.json";

/// 一条名句
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    /// 名句正文
    pub text: String,
    /// 出处（一般为「作者《篇名》」）
    #[serde(default)]
    pub source: String,
}

impl Quote {
    /// 拼成一行显示文本
    pub fn display(&self) -> String {
        if self.source.is_empty() {
            self.text.clone()
        } else {
            format!("{} —— {}", self.text, self.source)
        }
    }
}

/// 名句缓存
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cache {
    /// 抓取日期（YYYY-MM-DD）
    #[serde(default)]
    pub date: String,
    /// 该次抓取到的名句
    #[serde(default)]
    pub quotes: Vec<Quote>,
}

impl Cache {
    /// 读取缓存；文件缺失或损坏时返回空缓存
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// 写回缓存
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).ok();
        }
        let text = serde_json::to_string(self).context("序列化名句缓存失败")?;
        std::fs::write(path, text).with_context(|| format!("写入名句缓存失败: {}", path.display()))
    }

    /// 缓存是否来自今天
    fn is_today(&self) -> bool {
        !self.quotes.is_empty() && self.date == today_text()
    }
}

/// 获取今天要显示的名言。
///
/// 返回 `(显示文本, 可选日志)`。这里的逻辑不会失败，总能给出可显示的内容。
pub fn load_daily(cache_path: &Path) -> (String, Option<String>) {
    let mut cache = Cache::load(cache_path);
    let log;

    if cache.is_today() {
        log = Some(format!(
            "每日名言：沿用今日已缓存的 {} 条",
            cache.quotes.len()
        ));
    } else {
        match fetch() {
            Ok(quotes) => {
                let count = quotes.len();
                cache = Cache {
                    date: today_text(),
                    quotes,
                };
                log = Some(match cache.save(cache_path) {
                    Ok(()) => format!("每日名言：已从古文岛获取 {count} 条"),
                    Err(err) => format!("每日名言：已获取 {count} 条，但缓存写入失败（{err}）"),
                });
            }
            Err(err) => {
                log = Some(if cache.quotes.is_empty() {
                    format!("每日名言：获取失败，改用内置文案（{err}）")
                } else {
                    format!("每日名言：获取失败，沿用上次缓存（{err}）")
                });
            }
        }
    }

    (pick(&cache).display(), log)
}

/// 从缓存（或内置文案）中按日期挑选一条
fn pick(cache: &Cache) -> Quote {
    let index = day_of_year();
    if !cache.quotes.is_empty() {
        return cache.quotes[index % cache.quotes.len()].clone();
    }
    Quote {
        text: FALLBACK[index % FALLBACK.len()].to_string(),
        source: String::new(),
    }
}

/// 抓取并解析古文岛名句列表
pub fn fetch() -> Result<Vec<Quote>> {
    let agent = crate::http::agent(TIMEOUT);
    let mut response = agent
        .get(MINGJU_URL)
        .header("User-Agent", crate::http::BROWSER_UA)
        .call()
        .context("请求古文岛失败")?;
    let html = response
        .body_mut()
        .read_to_string()
        .context("读取古文岛页面失败")?;
    let quotes = parse(&html);
    if quotes.is_empty() {
        anyhow::bail!("页面未解析出名句，站点结构可能已变化");
    }
    Ok(quotes)
}

/// 解析页面中的名句区块。
///
/// 只认「名句链接(`/mingju/juv_…`) + 出处链接(`/shiwenv_…`)」这一对，
/// 以免把页面顶部的导航区块（如「古文岛 —— 推荐」）也当成名句。
fn parse(html: &str) -> Vec<Quote> {
    let document = Html::parse_document(html);
    let (Ok(block), Ok(link)) = (Selector::parse("div.cont"), Selector::parse("a")) else {
        return Vec::new();
    };
    let mut quotes = Vec::new();
    for node in document.select(&block) {
        let mut text = None;
        let mut source = None;
        for anchor in node.select(&link) {
            let href = anchor.value().attr("href").unwrap_or_default();
            let content = clean(&anchor.text().collect::<String>());
            if content.is_empty() {
                continue;
            }
            if text.is_none() && href.contains("/mingju/juv_") {
                text = Some(content);
            } else if source.is_none() && href.contains("/shiwenv_") {
                source = Some(content);
            }
        }
        if let Some(text) = text {
            quotes.push(Quote {
                text,
                source: source.unwrap_or_default(),
            });
        }
    }
    quotes
}

/// 去掉首尾空白与不换行空格
fn clean(text: &str) -> String {
    text.replace('\u{a0}', " ").trim().to_string()
}

/// 今天是一年中的第几天（用于轮换选句）
fn day_of_year() -> usize {
    chrono::Local::now().ordinal() as usize
}

/// 今天的日期文本（YYYY-MM-DD）
fn today_text() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 内置回退文案（网络不可用时使用）
const FALLBACK: [&str; 30] = [
    "天行健，君子以自强不息。——《周易》",
    "纸上得来终觉浅，绝知此事要躬行。——陆游",
    "宝剑锋从磨砺出，梅花香自苦寒来。",
    "不积跬步，无以至千里；不积小流，无以成江海。——荀子",
    "绳锯木断，水滴石穿。——《汉书》",
    "长风破浪会有时，直挂云帆济沧海。——李白",
    "有志者，事竟成。——《后汉书》",
    "博观而约取，厚积而薄发。——苏轼",
    "千磨万击还坚劲，任尔东西南北风。——郑燮",
    "路漫漫其修远兮，吾将上下而求索。——屈原",
    "骐骥一跃，不能十步；驽马十驾，功在不舍。——荀子",
    "盛年不重来，一日难再晨。——陶渊明",
    "会当凌绝顶，一览众山小。——杜甫",
    "锲而不舍，金石可镂。——荀子",
    "莫等闲，白了少年头，空悲切。——岳飞",
    "业精于勤，荒于嬉；行成于思，毁于随。——韩愈",
    "读书破万卷，下笔如有神。——杜甫",
    "黑发不知勤学早，白首方悔读书迟。——颜真卿",
    "志不强者智不达。——墨子",
    "天将降大任于是人也，必先苦其心志。——《孟子》",
    "少壮不努力，老大徒伤悲。——《长歌行》",
    "苟有恒，何必三更眠五更起。——颜真卿",
    "山重水复疑无路，柳暗花明又一村。——陆游",
    "积土成山，风雨兴焉；积水成渊，蛟龙生焉。——荀子",
    "纸上画饼终难饱，实干方能有所成。",
    "锲而不舍，朽木不折；锲而不舍，金石可镂。",
    "穷且益坚，不坠青云之志。——王勃",
    "宝剑不磨要生锈，人不学习要落后。",
    "天生我材必有用。——李白",
    "世上无难事，只要肯登攀。——毛泽东",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 与古文岛页面相同的结构片段
    const SAMPLE: &str = r#"
<div class="sons">
  <div class="cont" style="margin-top: 20px;">
    <a style="float: left;font-size:18px;" href="/mingju/juv_d42ce6f0d4f6.aspx">白日参辰现，北斗回南面。</a>
    <span style="color: #666666;">——&nbsp;</span><a href="/shiwenv_5b3ea1512f82.aspx">佚名《菩萨蛮&#183;枕前发尽千般愿》</a>
  </div>
  <div class="cont" style="margin-top: 20px;">
    <a href="/mingju/juv_22fdc3dbf1ee.aspx">富贵非吾事，归与白鸥盟。</a>
    <span>——&nbsp;</span><a href="/shiwenv_2eecfb5a442c.aspx">辛弃疾《水调歌头》</a>
  </div>
  <div class="cont"><a href="/x.aspx">只有一句没有出处</a></div>
  <div class="cont"><a href="/">古文岛</a><span>——</span><a href="/mingjus/default.aspx?xstr=%E6%8E%A8%E8%8D%90">推荐</a></div>
</div>"#;

    #[test]
    fn 解析出名句与出处() {
        let quotes = parse(SAMPLE);
        assert_eq!(quotes.len(), 2, "无名句链接的导航/残缺区块应被跳过");
        assert_eq!(quotes[0].text, "白日参辰现，北斗回南面。");
        assert_eq!(quotes[0].source, "佚名《菩萨蛮·枕前发尽千般愿》");
        assert_eq!(quotes[1].source, "辛弃疾《水调歌头》");
    }

    #[test]
    fn 显示文本带出处() {
        let quote = Quote {
            text: "会当凌绝顶".into(),
            source: "杜甫《望岳》".into(),
        };
        assert_eq!(quote.display(), "会当凌绝顶 —— 杜甫《望岳》");
        let plain = Quote {
            text: "只有正文".into(),
            source: String::new(),
        };
        assert_eq!(plain.display(), "只有正文");
    }

    #[test]
    fn 无网络时回退到内置文案() {
        let quote = pick(&Cache::default());
        assert!(!quote.text.is_empty());
        assert!(FALLBACK.contains(&quote.text.as_str()));
    }

    #[test]
    fn 同一天内选句稳定() {
        let cache = Cache {
            date: "2026-10-08".into(),
            quotes: vec![
                Quote {
                    text: "甲".into(),
                    source: "A".into(),
                },
                Quote {
                    text: "乙".into(),
                    source: "B".into(),
                },
            ],
        };
        assert_eq!(pick(&cache), pick(&cache));
    }

    #[test]
    fn 缓存可序列化往返() {
        let cache = Cache {
            date: "2026-10-08".into(),
            quotes: vec![Quote {
                text: "山重水复疑无路".into(),
                source: "陆游《游山西村》".into(),
            }],
        };
        let text = serde_json::to_string(&cache).unwrap();
        let back: Cache = serde_json::from_str(&text).unwrap();
        assert_eq!(back.date, cache.date);
        assert_eq!(back.quotes, cache.quotes);
    }
}
