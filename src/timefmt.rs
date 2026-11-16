//! 시간 표시 / 사용자 입력 해석

pub struct Parts {
    pub h: i64,
    pub m: i64,
    pub s: i64,
    pub cs: i64,
}

pub fn parts(ms: i64) -> Parts {
    let ms = ms.max(0);
    let t = ms / 1000;
    Parts { h: t / 3600, m: (t / 60) % 60, s: t % 60, cs: (ms / 10) % 100 }
}

/// "01:23:45" (+ ".67")
pub fn format(ms: i64, centis: bool) -> String {
    let p = parts(ms);
    if centis {
        format!("{:02}:{:02}:{:02}.{:02}", p.h, p.m, p.s, p.cs)
    } else {
        format!("{:02}:{:02}:{:02}", p.h, p.m, p.s)
    }
}

/// 편집창에 넣는 형태 "1:23:45"
pub fn format_edit(ms: i64) -> String {
    let p = parts(ms);
    format!("{}:{:02}:{:02}", p.h, p.m, p.s)
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Parsed {
    pub relative: bool,
    pub ms: i64,
}

/// 사용자 입력 해석
///  "1:30:00" → 1시간 30분, "1:30" → 1시간 30분(시:분), "45" → 45분
///  "90m" "1h20m" "1.5h" "30s" "2시간"
///  "+15" "-1:00" "+30s" → 현재 시간에서 더하기/빼기
pub fn parse(input: &str) -> Option<Parsed> {
    let mut s: String = input
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '−' || c == '–' { '-' } else { c })
        .collect::<String>()
        .to_lowercase()
        .replace('간', "");
    if s.is_empty() {
        return None;
    }
    let mut sign = 0i64;
    if s.starts_with('+') || s.starts_with('-') {
        sign = if s.starts_with('+') { 1 } else { -1 };
        s.remove(0);
    }
    if s.is_empty() {
        return None;
    }

    let ms: f64 = if s.contains(':') {
        let segs: Vec<&str> = s.split(':').collect();
        if segs.len() < 2 || segs.len() > 3 {
            return None;
        }
        let is_int = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit());
        let last = *segs.last().unwrap();
        let last_ok = {
            let mut it = last.splitn(2, '.');
            let a = it.next().unwrap_or("");
            let b = it.next();
            is_int(a) && a.len() <= 2 && b.map_or(true, |b| is_int(b) && segs.len() == 3)
        };
        if !last_ok || !is_int(segs[0]) || (segs.len() == 3 && !(is_int(segs[1]) && segs[1].len() <= 2)) {
            return None;
        }
        let lastv: f64 = last.parse().ok()?;
        if segs.len() == 3 {
            let h: f64 = segs[0].parse().ok()?;
            let m: f64 = segs[1].parse().ok()?;
            if m > 59.0 || lastv >= 60.0 {
                return None;
            }
            ((h * 60.0 + m) * 60.0 + lastv) * 1000.0
        } else {
            let h: f64 = segs[0].parse().ok()?;
            if lastv > 59.0 {
                return None;
            }
            (h * 60.0 + lastv) * 60_000.0
        }
    } else if s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        if s.matches('.').count() > 1 || s.starts_with('.') || s.ends_with('.') {
            return None;
        }
        s.parse::<f64>().ok()? * 60_000.0 // 숫자만 → 분
    } else {
        // 숫자+단위 반복
        let mut total = 0.0;
        let mut num = String::new();
        let mut any = false;
        for c in s.chars() {
            if c.is_ascii_digit() || c == '.' {
                num.push(c);
                continue;
            }
            let unit = match c {
                'h' | '시' => 3_600_000.0,
                'm' | '분' => 60_000.0,
                's' | '초' => 1000.0,
                _ => return None,
            };
            if num.is_empty() || num.matches('.').count() > 1 {
                return None;
            }
            total += num.parse::<f64>().ok()? * unit;
            num.clear();
            any = true;
        }
        if !num.is_empty() || !any {
            return None;
        }
        total
    };
    if !ms.is_finite() {
        return None;
    }
    let ms = ms.round() as i64;
    Some(if sign != 0 { Parsed { relative: true, ms: sign * ms } } else { Parsed { relative: false, ms } })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn abs(ms: i64) -> Option<Parsed> { Some(Parsed { relative: false, ms }) }
    fn rel(ms: i64) -> Option<Parsed> { Some(Parsed { relative: true, ms }) }

    #[test]
    fn parses() {
        assert_eq!(parse("1:30:00"), abs(5_400_000));
        assert_eq!(parse("1:30"), abs(5_400_000));
        assert_eq!(parse("45"), abs(2_700_000));
        assert_eq!(parse("+10"), rel(600_000));
        assert_eq!(parse("-5m"), rel(-300_000));
        assert_eq!(parse("−5m"), rel(-300_000));
        assert_eq!(parse("1h20m"), abs(4_800_000));
        assert_eq!(parse("1.5h"), abs(5_400_000));
        assert_eq!(parse("30s"), abs(30_000));
        assert_eq!(parse("2시간"), abs(7_200_000));
        assert_eq!(parse("1시간 30분"), abs(5_400_000));
        assert_eq!(parse("+1:00"), rel(3_600_000));
        assert_eq!(parse("0:00:05.5"), abs(5_500));
        assert_eq!(parse("abc"), None);
        assert_eq!(parse("1:75"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("+"), None);
        assert_eq!(parse("1:2:3:4"), None);
        assert_eq!(parse("5x"), None);
    }

    #[test]
    fn formats() {
        assert_eq!(format(3_723_456, true), "01:02:03.45");
        assert_eq!(format(3_723_456, false), "01:02:03");
        assert_eq!(format_edit(3_723_456), "1:02:03");
        assert_eq!(format(-5, false), "00:00:00");
    }
}
