//! 插件机制 — RAGFlow `agent/plugin` 的 Rust 实现
//!
//! 对齐 plugin_manager.py / llm_tool_plugin.py / common.py / embedded_plugins：
//!   - 插件类型常量（common.py `PLUGIN_TYPE_LLM_TOOLS = "llm_tools"`）
//!   - [`LLMToolPlugin`] 契约：`metadata()`（对齐 classmethod get_metadata()）+
//!     `invoke(arguments, env) -> String`（对齐 invoke(**kwargs) -> str）
//!   - [`llm_tool_metadata_to_openai_tool`]：LLM 工具元数据 → OpenAI function tool
//!   - [`PluginManager`]：按名称注册/查询（对齐 get_llm_tools / get_llm_tool_by_name /
//!     get_llm_tools_by_names）+ 内置插件加载（对齐 load_plugins 的 embedded_plugins
//!     目录语义）+ 环境变量注入（[`PluginEnv`]）+ 错误隔离（单插件失败/panic 不
//!     影响管理器与其他插件）
//!   - 内置插件：`bad_calculator`（忠实复刻 embedded_plugins/llm_tools/
//!     bad_calculator.py）+ `text_reverse`（RayRAG 演示扩展，上游无对应）

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// 插件类型：LLM 工具（对齐 common.py `PLUGIN_TYPE_LLM_TOOLS = "llm_tools"`）。
pub const PLUGIN_TYPE_LLM_TOOLS: &str = "llm_tools";

/// 内置插件目录名（对齐 plugin_manager.py 的 embedded_plugins 扫描路径）。
pub const EMBEDDED_PLUGINS_DIR: &str = "embedded_plugins";

/// LLM 工具参数（对齐 llm_tool_plugin.py `LLMToolParameter`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LLMToolParameter {
    #[serde(rename = "type")]
    pub r#type: String,
    pub description: String,
    #[serde(
        rename = "displayDescription",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub display_description: Option<String>,
    pub required: bool,
}

impl LLMToolParameter {
    pub fn new(r#type: impl Into<String>, description: impl Into<String>, required: bool) -> Self {
        Self {
            r#type: r#type.into(),
            description: description.into(),
            display_description: None,
            required,
        }
    }
}

/// LLM 工具元数据（对齐 llm_tool_plugin.py `LLMToolMetadata`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LLMToolMetadata {
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub description: String,
    #[serde(
        rename = "displayDescription",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub display_description: Option<String>,
    /// 参数表（保序，对齐 dict[str, LLMToolParameter] 的插入序）。
    pub parameters: Vec<(String, LLMToolParameter)>,
}

impl LLMToolMetadata {
    /// 取参数（对齐 dict 下标语义）。
    pub fn get_parameter(&self, name: &str) -> Option<&LLMToolParameter> {
        self.parameters
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, parameter)| parameter)
    }
}

/// 插件执行环境（环境变量注入）。
///
/// 管理器在调用插件前把选定环境变量快照注入调用上下文；插件经 [`PluginEnv::get`]
/// 读取。不做全局 `std::env::set_var`（edition 2024 下为 unsafe，且全局可变有悖并发）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginEnv {
    vars: HashMap<String, String>,
}

impl PluginEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.vars.insert(key.into(), value.into());
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// 从进程环境快照（可选前缀过滤，供插件挑选其专属变量）。
    pub fn snapshot(prefix: Option<&str>) -> Self {
        let mut vars = HashMap::new();
        for (key, value) in std::env::vars() {
            if let Some(prefix) = prefix
                && !key.starts_with(prefix) {
                    continue;
                }
            vars.insert(key, value);
        }
        Self { vars }
    }
}

/// LLM 工具插件（对齐 `LLMToolPlugin` 基类：get_metadata + invoke(**kwargs) -> str）。
pub trait LLMToolPlugin: Send + Sync {
    /// 插件版本（对齐 pluginlib `_version_`，默认 "1.0.0"）。
    fn version(&self) -> &str {
        "1.0.0"
    }

    /// 插件元数据（对齐 classmethod get_metadata()）。
    fn metadata(&self) -> &LLMToolMetadata;

    /// 调用插件（对齐 invoke(**kwargs) -> str）。
    ///
    /// 参数缺失/类型不符或实现错误 → `Err`；由 [`PluginManager`] 隔离，
    /// 不会污染管理器或其他插件。
    fn invoke(&self, arguments: &Map<String, Value>, env: &PluginEnv) -> Result<String>;
}

/// LLM 工具元数据 → OpenAI function tool（对齐 llm_tool_metadata_to_openai_tool）。
pub fn llm_tool_metadata_to_openai_tool(metadata: &LLMToolMetadata) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, parameter) in &metadata.parameters {
        properties.insert(
            name.clone(),
            json!({
                "type": parameter.r#type,
                "description": parameter.description,
            }),
        );
        if parameter.required {
            required.push(name.clone());
        }
    }
    json!({
        "type": "function",
        "function": {
            "name": metadata.name,
            "description": metadata.description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            },
        },
    })
}

/// 插件管理器（对齐 plugin_manager.py `PluginManager`）。
#[derive(Clone, Default)]
pub struct PluginManager {
    llm_tool_plugins: HashMap<String, Arc<dyn LLMToolPlugin>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册插件：按元数据 `name` 注册（对齐 load_plugins 的
    /// `self._llm_tool_plugins[metadata["name"]] = plugin`）。
    pub fn register_plugin(&mut self, plugin: Arc<dyn LLMToolPlugin>) {
        let name = plugin.metadata().name.clone();
        self.llm_tool_plugins.insert(name, plugin);
    }

    /// 加载内置插件（对齐 load_plugins：扫描 embedded_plugins 目录并注册）。
    ///
    /// 当前注册 [`BadCalculatorPlugin`]（上游 embedded_plugins 唯一内置）与
    /// [`TextReversePlugin`]（RayRAG 演示扩展）。
    pub fn load_embedded_plugins(&mut self) {
        self.register_plugin(Arc::new(BadCalculatorPlugin::new()));
        self.register_plugin(Arc::new(TextReversePlugin::new()));
    }

    /// 全部 LLM 工具插件（对齐 get_llm_tools）。
    pub fn get_llm_tools(&self) -> Vec<Arc<dyn LLMToolPlugin>> {
        self.llm_tool_plugins.values().cloned().collect()
    }

    /// 按名称取插件（对齐 get_llm_tool_by_name）。
    pub fn get_llm_tool_by_name(&self, name: &str) -> Option<Arc<dyn LLMToolPlugin>> {
        self.llm_tool_plugins.get(name).cloned()
    }

    /// 按名称列表取插件（对齐 get_llm_tools_by_names：缺失名称静默跳过）。
    pub fn get_llm_tools_by_names(&self, tool_names: &[String]) -> Vec<Arc<dyn LLMToolPlugin>> {
        tool_names
            .iter()
            .filter_map(|name| self.llm_tool_plugins.get(name).cloned())
            .collect()
    }

    /// 调用 LLM 工具（错误隔离：未知名称 → Err；插件返回 Err → 透传；
    /// 插件 panic → 捕获并转 Err；均不影响管理器后续使用）。
    pub fn invoke_llm_tool(
        &self,
        name: &str,
        arguments: &Map<String, Value>,
        env: &PluginEnv,
    ) -> Result<String> {
        let plugin = self
            .get_llm_tool_by_name(name)
            .ok_or_else(|| anyhow::anyhow!("LLM tool plugin not found: {name}"))?;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            plugin.invoke(arguments, env)
        })) {
            Ok(inner) => inner,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|message| (*message).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic payload".to_string());
                Err(anyhow::anyhow!("LLM tool plugin panicked: {message}"))
            }
        }
    }
}

/// 内置插件：bad_calculator（忠实复刻 embedded_plugins/llm_tools/bad_calculator.py）。
///
/// 演示用插件：两数相加再加 100（故意给出错误答案），勿用于生产。
#[derive(Debug, Clone)]
pub struct BadCalculatorPlugin {
    metadata: LLMToolMetadata,
}

impl BadCalculatorPlugin {
    pub fn new() -> Self {
        Self {
            metadata: LLMToolMetadata {
                name: "bad_calculator".to_string(),
                display_name: "$t:bad_calculator.name".to_string(),
                description: "A tool to calculate the sum of two numbers (will give wrong answer)"
                    .to_string(),
                display_description: Some("$t:bad_calculator.description".to_string()),
                parameters: vec![
                    (
                        "a".to_string(),
                        LLMToolParameter::new("number", "The first number", true),
                    ),
                    (
                        "b".to_string(),
                        LLMToolParameter::new("number", "The second number", true),
                    ),
                ],
            },
        }
    }
}

impl Default for BadCalculatorPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl LLMToolPlugin for BadCalculatorPlugin {
    fn version(&self) -> &str {
        "1.0.0"
    }

    fn metadata(&self) -> &LLMToolMetadata {
        &self.metadata
    }

    fn invoke(&self, arguments: &Map<String, Value>, _env: &PluginEnv) -> Result<String> {
        let a = arguments
            .get("a")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("Parameter 'a' is required and must be a number"))?;
        let b = arguments
            .get("b")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("Parameter 'b' is required and must be a number"))?;
        Ok((a + b + 100.0).to_string())
    }
}

/// 内置插件（RayRAG 演示扩展，上游无对应）：text_reverse — 反转字符串。
#[derive(Debug, Clone)]
pub struct TextReversePlugin {
    metadata: LLMToolMetadata,
}

impl TextReversePlugin {
    pub fn new() -> Self {
        Self {
            metadata: LLMToolMetadata {
                name: "text_reverse".to_string(),
                display_name: "Text Reverse".to_string(),
                description: "Reverse the given text".to_string(),
                display_description: None,
                parameters: vec![(
                    "text".to_string(),
                    LLMToolParameter::new("string", "The text to reverse", true),
                )],
            },
        }
    }
}

impl Default for TextReversePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl LLMToolPlugin for TextReversePlugin {
    fn version(&self) -> &str {
        "1.0.0"
    }

    fn metadata(&self) -> &LLMToolMetadata {
        &self.metadata
    }

    fn invoke(&self, arguments: &Map<String, Value>, _env: &PluginEnv) -> Result<String> {
        let text = arguments
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Parameter 'text' is required and must be a string"))?;
        Ok(text.chars().rev().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manager_with_embedded() -> PluginManager {
        let mut manager = PluginManager::new();
        manager.load_embedded_plugins();
        manager
    }

    #[test]
    fn embedded_plugins_register_and_query_by_name() {
        let manager = manager_with_embedded();
        assert!(manager.get_llm_tool_by_name("bad_calculator").is_some());
        assert!(manager.get_llm_tool_by_name("text_reverse").is_some());
        assert!(manager.get_llm_tool_by_name("missing").is_none());
        assert_eq!(manager.get_llm_tools().len(), 2);
        // 按名称列表过滤（对齐 get_llm_tools_by_names：缺失名称跳过）
        let tools =
            manager.get_llm_tools_by_names(&["bad_calculator".to_string(), "missing".to_string()]);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].metadata().name.as_str(), "bad_calculator");
    }

    #[test]
    fn bad_calculator_invoke_matches_upstream_and_isolates_errors() {
        let manager = manager_with_embedded();
        let env = PluginEnv::new();
        // 1 + 2 + 100 = 103（对齐 bad_calculator.py：str(a + b + 100)）
        let args = Map::from_iter([("a".into(), json!(1)), ("b".into(), json!(2))]);
        assert_eq!(
            manager
                .invoke_llm_tool("bad_calculator", &args, &env)
                .unwrap(),
            "103"
        );
        // 缺参 → Err（错误隔离，不 panic）
        let error = manager
            .invoke_llm_tool("bad_calculator", &Map::new(), &env)
            .unwrap_err()
            .to_string();
        assert!(error.contains("'a' is required"));
        // 失败后管理器与其他插件不受影响
        let args = Map::from_iter([("text".into(), json!("abc"))]);
        assert_eq!(
            manager
                .invoke_llm_tool("text_reverse", &args, &env)
                .unwrap(),
            "cba"
        );
        // 未知插件名 → Err
        assert!(manager.invoke_llm_tool("nope", &Map::new(), &env).is_err());
    }

    #[test]
    fn metadata_to_openai_tool_matches_contract() {
        let manager = manager_with_embedded();
        let plugin = manager.get_llm_tool_by_name("bad_calculator").unwrap();
        assert_eq!(plugin.version(), "1.0.0");
        let tool = llm_tool_metadata_to_openai_tool(plugin.metadata());
        assert_eq!(tool["type"].as_str(), Some("function"));
        assert_eq!(tool["function"]["name"].as_str(), Some("bad_calculator"));
        assert_eq!(
            tool["function"]["description"].as_str(),
            Some("A tool to calculate the sum of two numbers (will give wrong answer)")
        );
        assert_eq!(
            tool["function"]["parameters"]["type"].as_str(),
            Some("object")
        );
        assert_eq!(
            tool["function"]["parameters"]["properties"]["a"]["type"].as_str(),
            Some("number")
        );
        let required = tool["function"]["parameters"]["required"]
            .as_array()
            .unwrap();
        assert!(required.contains(&json!("a")) && required.contains(&json!("b")));
        // 参数元数据（对齐 LLMToolParameter / LLMToolMetadata 语义）
        let metadata = plugin.metadata();
        assert!(metadata.get_parameter("a").unwrap().required);
        assert_eq!(
            metadata.get_parameter("b").unwrap().r#type.as_str(),
            "number"
        );
        assert_eq!(metadata.display_name.as_str(), "$t:bad_calculator.name");
        assert_eq!(metadata.get_parameter("z"), None);
    }

    #[test]
    fn env_injection_reaches_plugin_and_snapshot_filters() {
        // 测试插件：读取注入的 PluginEnv
        struct SuffixPlugin {
            metadata: LLMToolMetadata,
        }
        impl LLMToolPlugin for SuffixPlugin {
            fn metadata(&self) -> &LLMToolMetadata {
                &self.metadata
            }
            fn invoke(&self, arguments: &Map<String, Value>, env: &PluginEnv) -> Result<String> {
                let text = arguments.get("text").and_then(Value::as_str).unwrap_or("");
                let suffix = env.get("RAYRAG_TEST_SUFFIX").unwrap_or("");
                Ok(format!("{text}{suffix}"))
            }
        }
        let mut manager = PluginManager::new();
        manager.register_plugin(Arc::new(SuffixPlugin {
            metadata: LLMToolMetadata {
                name: "suffix".to_string(),
                display_name: "Suffix".to_string(),
                description: "Append env-injected suffix".to_string(),
                display_description: None,
                parameters: Vec::new(),
            },
        }));
        let mut env = PluginEnv::new();
        env.insert("RAYRAG_TEST_SUFFIX", "!");
        let args = Map::from_iter([("text".into(), json!("hi"))]);
        assert_eq!(
            manager.invoke_llm_tool("suffix", &args, &env).unwrap(),
            "hi!"
        );
        // 全量快照包含进程环境（如 PATH/HOME）；不存在的前缀 → 空快照
        let snapshot = PluginEnv::snapshot(None);
        assert!(snapshot.get("PATH").is_some() || snapshot.get("HOME").is_some());
        assert_eq!(
            PluginEnv::snapshot(Some("RAYRAG_NO_SUCH_PREFIX_")),
            PluginEnv::new()
        );
    }

    #[test]
    fn plugin_panic_is_isolated_from_manager() {
        struct PanicPlugin {
            metadata: LLMToolMetadata,
        }
        impl LLMToolPlugin for PanicPlugin {
            fn metadata(&self) -> &LLMToolMetadata {
                &self.metadata
            }
            fn invoke(&self, _arguments: &Map<String, Value>, _env: &PluginEnv) -> Result<String> {
                panic!("boom")
            }
        }
        let mut manager = PluginManager::new();
        manager.register_plugin(Arc::new(PanicPlugin {
            metadata: LLMToolMetadata {
                name: "panic".to_string(),
                display_name: "Panic".to_string(),
                description: "Always panics".to_string(),
                display_description: None,
                parameters: Vec::new(),
            },
        }));
        let error = manager
            .invoke_llm_tool("panic", &Map::new(), &PluginEnv::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("panicked"));
        // 管理器未被污染：加载内置插件后照常工作
        manager.load_embedded_plugins();
        let args = Map::from_iter([("text".into(), json!("ab"))]);
        assert_eq!(
            manager
                .invoke_llm_tool("text_reverse", &args, &PluginEnv::new())
                .unwrap(),
            "ba"
        );
    }
}
