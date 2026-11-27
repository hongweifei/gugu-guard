//! 未知配置键的检出与拼写建议。

use super::AppConfig;

/// 找出配置文件里有、但结构体不认识的键。
///
/// 做法是把「原始 TOML」与「按结构体序列化出的已知键集合」逐层比对：
/// 后者用 JSON 承载，因为 TOML 序列化会略过 `None` 字段，JSON 会把它们保留成 `null`，
/// 这样字段名清单才是完整的，拼写建议也才有候选。相比 `serde_ignored`，
/// 这条路对 `#[serde(flatten)]` 与内部标签枚举同样准确。
pub(super) fn collect(content: &str, config: &AppConfig) -> Vec<String> {
    let Ok(original) = toml::from_str::<toml::Value>(content) else {
        return Vec::new();
    };
    let Ok(known) = serde_json::to_value(config) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    diff_keys("", &original, &known, &mut found);
    found
}

fn diff_keys(
    prefix: &str,
    original: &toml::Value,
    known: &serde_json::Value,
    found: &mut Vec<String>,
) {
    match (original, known) {
        (toml::Value::Table(original), serde_json::Value::Object(known)) => {
            for (key, value) in original {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match known.get(key) {
                    Some(known_value) => diff_keys(&path, value, known_value, found),
                    None => found.push(match suggest(key, known.keys()) {
                        Some(hint) => format!("{path}（是否想写 {hint}）"),
                        None => path,
                    }),
                }
            }
        }
        (toml::Value::Array(original), serde_json::Value::Array(known)) => {
            for (index, value) in original.iter().enumerate() {
                if let Some(known_value) = known.get(index) {
                    diff_keys(&format!("{prefix}[{index}]"), value, known_value, found);
                }
            }
        }
        _ => {}
    }
}

/// 在同一层的已知键里找一个最接近的拼写，距离太远就不猜。
fn suggest<'a>(unknown: &str, candidates: impl Iterator<Item = &'a String>) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        let distance = edit_distance(unknown, candidate);
        if distance == 0 {
            continue;
        }
        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, candidate));
        }
    }
    let (distance, candidate) = best?;
    let longest = unknown.chars().count().max(candidate.chars().count());
    let limit = if longest <= 6 { 2 } else { 3 };
    (distance <= limit).then_some(candidate)
}

fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (row, left_char) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right.iter().enumerate() {
            let substitute = previous[column] + usize::from(left_char != right_char);
            current[column + 1] = substitute
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}
