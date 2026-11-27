//! 写回配置时与原文件底稿的合并：保留注释与未变动条目的版式。

use toml_edit::{Item, Table};

/// 把 `src` (新配置序列化结果) 的值覆盖进 `dst` (原文件底稿)：
/// - 双方都是表：递归合并
/// - 已存在的标量键：替换值，保留其行前/行尾注释与空白 (decor)
/// - 其余情况 (新增/类型变化)：整体替换
/// - `dst` 中 `src` 不再包含的键：移除 (配置模型是唯一事实，删除的进程/字段不残留)
pub(super) fn merge_tables(dst: &mut Table, src: &Table) {
    let stale: Vec<String> = dst
        .iter()
        .filter(|(k, _)| src.get(k).is_none())
        .map(|(k, _)| k.to_string())
        .collect();
    for k in &stale {
        dst.remove(k);
    }
    for (k, sv) in src.iter() {
        if sv.is_table() && dst.get(k).is_some_and(Item::is_table) {
            if let (Some(dt), Some(st)) =
                (dst.get_mut(k).and_then(|i| i.as_table_mut()), sv.as_table())
            {
                merge_tables(dt, st);
            }
            continue;
        }
        let old_key_decor = dst.key(k).map(|key| key.leaf_decor().clone());
        let old_val_decor = dst
            .get(k)
            .and_then(|i| i.as_value())
            .map(|v| (v.decor().prefix().cloned(), v.decor().suffix().cloned()));
        let mut new_item = sv.clone();
        if let (Some((p, s)), Some(nv)) = (old_val_decor, new_item.as_value_mut()) {
            let mut decor = toml_edit::Decor::default();
            if let Some(p) = p {
                decor.set_prefix(p);
            }
            if let Some(s) = s {
                decor.set_suffix(s);
            }
            *nv.decor_mut() = decor;
        }
        dst.insert(k, new_item);
        if let Some(kd) = old_key_decor {
            if let Some(mut key) = dst.key_mut(k) {
                *key.leaf_decor_mut() = kd;
            }
        }
    }
}
