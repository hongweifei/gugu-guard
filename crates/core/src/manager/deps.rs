//! 依赖拓扑排序：按 `depends_on` 计算启动顺序，检测循环依赖。

use crate::config::ProcessConfig;
use crate::error::{GuguError, Result};
use std::collections::{HashMap, HashSet, VecDeque};

/// 按 `depends_on` 计算启动顺序（被依赖者在前）。
///
/// # Errors
/// 存在循环依赖时返回 `GuguError::CyclicDependency`（消息里带环路成员）。
pub fn topological_sort(processes: &HashMap<String, ProcessConfig>) -> Result<Vec<String>> {
    let mut in_degree: HashMap<&str, u32> = HashMap::with_capacity(processes.len());
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::with_capacity(processes.len());

    for name in processes.keys() {
        in_degree.insert(name.as_str(), 0);
        dependents.entry(name.as_str()).or_default();
    }

    for (name, config) in processes {
        for dep in &config.depends_on {
            if !processes.contains_key(dep) {
                tracing::warn!("[{}] 依赖的 '{}' 不存在，忽略", name, dep);
                continue;
            }
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(name.as_str());
            // SAFETY: 所有 processes 的 key 已在上方初始化到 in_degree
            *in_degree.get_mut(name.as_str()).unwrap() += 1;
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&name, _)| name)
        .collect();

    let mut result = Vec::with_capacity(processes.len());
    while let Some(name) = queue.pop_front() {
        result.push(name.to_string());
        if let Some(deps) = dependents.get(name) {
            for &dep in deps {
                if let Some(degree) = in_degree.get_mut(dep) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(dep);
                    }
                }
            }
        }
    }

    if result.len() != processes.len() {
        let result_set: HashSet<&str> = result.iter().map(String::as_str).collect();
        let remaining: Vec<&str> = processes
            .keys()
            .filter(|k| !result_set.contains(k.as_str()))
            .map(String::as_str)
            .collect();
        return Err(GuguError::CyclicDependency(format!(
            "检测到循环依赖: {}",
            remaining.join(", ")
        )));
    }

    Ok(result)
}
