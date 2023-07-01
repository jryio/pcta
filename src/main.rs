use anyhow::Context;
use chrono::NaiveDate;
use rand::rngs::ThreadRng;
use rand::Rng;
use reqwest::header::{CACHE_CONTROL, PRAGMA, USER_AGENT};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt::Write;
use std::io::{self, Write as WRT};
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;
use ua_generator::ua::spoof_ua;

const URL: &str = "https://portal.permit.pcta.org/availability/mexican-border.php";
const PERIOD_MIN: u64 = 24; /* 15 seconds */
const PERIOD_MAX: u64 = 40; /* 60 seconds */
const LIMIT: u64 = 50;
const RANGE_YEAR: i32 = 2023;

// 2023-04-01
const RANGE_MONTH_START: u32 = 4;
const RANGE_DAY_START: u32 = 1;

// 2023-05-05
const RANGE_MONTH_END: u32 = 5;
const RANGE_DAY_END: u32 = 5;

#[derive(Serialize, Deserialize)]
struct Channel {
    name: String,
}

#[derive(Serialize, Deserialize)]
struct Message {
    body: String,
}

#[derive(Serialize, Deserialize)]
struct Options {
    channel: Channel,
    message: Message,
}

#[derive(Serialize, Deserialize)]
struct Params {
    options: Options,
}

#[derive(Serialize, Deserialize)]
struct KeybaseApi {
    method: String,
    params: Params,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    // YYYY-MM-DD
    start_date: String,
    // Actually is a u64
    num: String,
}

#[derive(Serialize, Deserialize)]
struct Data {
    limit: u64,
    calendar: Vec<Entry>,
}

pub async fn scrape(client: &Client) -> anyhow::Result<Vec<(NaiveDate, u64)>> {
    let ua = spoof_ua();
    let response = client


















        .get(URL)
        .header(USER_AGENT, ua)
        .header(PRAGMA, "no-cache")
        .header(CACHE_CONTROL, "no-cache")
        .send()
        .await?;
    let text = response.text().await?;

    let html = scraper::Html::parse_document(&text);
    let script_selector =
        scraper::Selector::parse(".container > script[type='text/javascript']").unwrap();

    let re = regex::Regex::new(r"var data = (\{.*\});").unwrap();
    let script = html.select(&script_selector).next().context(
        "Failed to select <script> tag in HTML document. We may be getting IP blocked or CAPTCHA",
    )?;
    // println!("{:?}", script.inner_html());
    let inner_html = script.inner_html();

    let caps = re.captures_iter(&inner_html).next().unwrap();
    println!("DEBUG DEBUG DEBUG \n\n{:?}", &caps[1]);
    let data_str = &caps[1];

    let data = serde_json::from_str::<Data>(data_str)
        .context("We parsed Invalid JSON from the PCTA <script> tag, investiagate the script tag or the regex result")?;

    // CONFIGURATION
    // I want to find dates which start after March 13 and before April 20th
    //
    // I want to be notified if/when any date in this range has a number of permits whihc is less
    // than the `limit`
    let range_start =
        NaiveDate::from_ymd_opt(RANGE_YEAR, RANGE_MONTH_START, RANGE_DAY_START).unwrap();
    let range_end = NaiveDate::from_ymd_opt(RANGE_YEAR, RANGE_MONTH_END, RANGE_DAY_END).unwrap();

    let mut results: Vec<(NaiveDate, u64)> = vec![];

    for entry in data.calendar {
        let start_date_fmt = "%Y-%m-%d";
        let entry_date = chrono::NaiveDate::parse_from_str(&entry.start_date, start_date_fmt)
            .with_context(|| {
                format!(
                    "Invalid 'start_date' string from PCTA = '{}', does not match {}",
                    entry.start_date, &start_date_fmt
                )
            })?;
        let entry_num = entry.num.parse::<u64>().with_context(|| {
            format!(
                "Invalid 'num' string from PCTA = '{}' on start_date = '{}'",
                entry.num, entry.start_date
            )
        })?;

        // should return the date which has < 50 numbers here
        if entry_date.gt(&range_start) && entry_date.le(&range_end) && entry_num < LIMIT {
            results.push((entry_date, entry_num))
        }
    }

    Ok(results)
}

pub fn handle_result(
    res: &anyhow::Result<Vec<(NaiveDate, u64)>>,
    now: &String,
) -> anyhow::Result<serde_json::Value> {
    match res {
        Ok(open_dates) => {
            let mut msg = String::new();
            let json = match open_dates.is_empty() {
                true => {
                    write!(
                        &mut msg,
                        "`{}` @ There are zero available permits in the date range",
                        now
                    )?;
                    json!({
                        "method": "send",
                        "params": {
                            "options": {
                                "channel": {
                                    "name": "jry.zed",
                                    "members_type": "team",
                                    "topic_name": "pcta-logs",
                                },
                                "message": {
                                    "body": msg,
                                }
                            }
                        }
                    })
                }
                false => {
                    write!(
                        &mut msg,
                        "@jacobyoung - *There are {} NEW starting dates open!*\n\n",
                        open_dates.len()
                    )?;

                    for (date, num) in open_dates {
                        writeln!(&mut msg, "* `{}`: {}", date, LIMIT - num)?;
                    }
                    writeln!(&mut msg, "\n`{}` - Scrape time", now)?;
                    json!({
                        "method": "send",
                        "params": {
                            "options": {
                                "channel": {
                                    "name": "jry.zed",
                                    "members_type": "team",
                                    "topic_name": "pcta-alerts",
                                },
                                "message": {
                                    "body": msg,
                                }
                            }
                        }
                    })
                }
            };

            println!("{}", msg);
            Ok(json)
        }
        Err(e) => {
            let msg = format!(
                "Failed to scrape PCTA page with error = \n\n```\n{}\n```\n",
                e
            );
            println!("{}", msg);
            let json = json!({
                "method": "send",
                "params": {
                    "options": {
                        "channel": {
                            "name": "jry.zed",
                            "members_type": "team",
                            "topic_name": "pcta-errors",
                        },
                        "message": {
                            "body": msg,
                        }
                    }
                }
            });
            Ok(json)
        }
    }
}

pub async fn loop_scrape(client: Client) -> anyhow::Result<()> {
    // Initialize each scraper with a different interval to prevent detection of scraping
    let num = (rand::random::<u64>() % (PERIOD_MAX + PERIOD_MIN)) + PERIOD_MIN;
    let rand_interval = num.clamp(PERIOD_MIN, PERIOD_MAX);
    println!("{} - Second Interval Initalized", rand_interval);
    let mut interval = tokio::time::interval(Duration::from_secs(rand_interval));

    loop {
        interval.tick().await;

        let now = chrono::offset::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let now_time = chrono::offset::Local::now().naive_local().time();
        // 9 AM PST -> 12 PM EST
        let start = chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        // 5 PM PST -> 8 PM EST
        let end = chrono::NaiveTime::from_hms_opt(20, 0, 0).unwrap();

        // We are not in business hours, don't scrape
        if now_time < start || now_time > end {
            let duration = now_time - start;
            let seconds = duration.num_seconds() % 60;
            let minutes = (duration.num_seconds() / 60) % 60;
            let hours = (duration.num_seconds() / 60) / 60;
            let msg = format!(
                "Not scraping since we're before business hours 9AM - 5PM PST. Next scrape in : {}h {}m {}s",
                hours, minutes, seconds
            );
            println!("{}", msg);

            let msg_json = json!({
                "method": "send",
                "params": {
                    "options": {
                        "channel": {
                            "name": "jry.zed",
                            "members_type": "team",
                            "topic_name": "pcta-logs",
                        },
                        "message": {
                            "body": msg,
                        }
                    }
                }
            });
            let mut keybase = Command::new("keybase");
            keybase
                .arg("chat")
                .arg("api")
                .arg("-m")
                .arg(msg_json.to_string())
                .spawn()
                .expect("Failed to call keybase API process (err)");
            continue;
        }

        let res = scrape(&client).await;
        let msg_json = handle_result(&res, &now)?;
        let mut keybase = Command::new("keybase");
        keybase
            .arg("chat")
            .arg("api")
            .arg("-m")
            .arg(msg_json.to_string())
            .spawn()
            .expect("Failed to call keybase API process (err)");

        println!("{} - Completed a scrape of PCTA site", now);

        // Reconnect to the VPN to try and get around IP blocking
        if res.is_err() {
            let mut mullvad = Command::new("mullvad");
            mullvad.arg("reconnect").spawn().expect("Failed to call mullvad reconnect (err)");
            let msg = format!("`{}` - *Reconnected to the VPN*", now);
            println!("{}", msg);
            let msg_json = json!({
                "method": "send",
                "params": {
                    "options": {
                        "channel": {
                            "name": "jry.zed",
                            "members_type": "team",
                            "topic_name": "pcta-logs",
                        },
                        "message": {
                            "body": msg,
                        }
                    }
                }
            });
            let mut keybase = Command::new("keybase");
            keybase
                .arg("chat")
                .arg("api")
                .arg("-m")
                .arg(msg_json.to_string())
                .spawn()
                .expect("Failed to call keybase API process (err)");
        }


        println!("{} - {} - Seconds until next scrape", now, rand_interval);
    }
}

pub async fn init_vpn() -> anyhow::Result<()> {
    todo!();
}

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    // Establish connection on the mullvad VPN to prevent IP scrape detection.
    //
    // `mullvad relay set location us`                          - Allows for selection of relays in the United States
    // `mullvad relay set tunnel wireguard --entry-location us` - Uses the WireGuard protocol through US relays
    // `mullvad relay get`                                      - Returns the relay configuration
    //
    // `mullvad connect`                                        - Connects using the relay config
    // `mullvad reconnect`                                      - Forces a reconnection at a new location
    // `mullvad status`                                         - 'Connected' or 'Disconnected' appear in output
    // `mullvad disconnect`                                     - Disconnects from the relay

    // TODO: Implement this
    // init_vpn();

    // Loop here
    let forever = tokio::task::spawn(loop_scrape(client));

    // Start
    forever.await??;

    // Never exit
    Ok(())
}
