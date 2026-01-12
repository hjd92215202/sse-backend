# SSE 平台后端架构设计说明书 (Part 1: Backend)

## 1. 核心设计理念
后端架构遵循 **“确定性优先，智能进化为辅”**。使用 Rust 的强类型系统管理复杂的语义状态机，确保每一个生成的查询都有据可查，通过异步任务在不影响用户体验的情况下实现系统的自我迭代。

## 2. 技术栈选型 
*   **Web 框架**: `Axum` (基于 Tokio，高性能异步处理)。
*   **语义索引**: `fst` (Finite State Transducers, 用于毫秒级实体模糊匹配)。
*   **状态管理**: `DashMap` (高并发内存 Session 存储)。
*   **持久层**: `PostgreSQL` (存储本体映射、用户信息及对话链路)。
*   **向量检索**: `Qdrant` (用于 Schema 的辅助语义寻找)。
*   **推理引擎**: `Oxigraph` 或自定义 `SPARQL` 生成器。
*   **异步任务**: `Tokio::spawn` 驱动的影子执行流水线。

## 3. 核心功能组件

### 3.1 语义推断引擎 (Inference Engine)
*   **FST 词库加载器**: 系统启动时，从数据库读取人工标注的“实体-别名”映射，编译为 FST 字节流。
*   **双径匹配算法**:
    *   **路径 A (Exact)**: 完全匹配实体名。
    *   **路径 B (Fuzzy)**: 基于 Levenshtein 距离，识别拼写错误或近似口语。
*   **歧义判定器**: 若一次 Query 匹配到多个属于不同维度的实体，判定为 `Ambiguous`，并将结果推入状态机。

### 3.2 意图状态机 (Intent State Machine)
*   **Context 维护**: 记录 Session 级别的实体槽位信息。
*   **转移逻辑**:
    *   `New` -> `Inferred` (推断中)
    *   `Inferred` -> `Clarifying` (存在歧义，下发反问)
    *   `Clarifying` -> `Confirmed` (用户确认，进入执行)
    *   `Confirmed` -> `Executed` (结果返回)

### 3.3 物理映射执行器 (Physical Executor)
*   **SQL 组装器**: 根据人工标注的 `SemanticMapping` 配置，将实体映射为物理表名、字段名和聚合函数（如 `SUM`, `COUNT`）。
*   **多源适配**: 封装 `sqlx` 驱动，支持跨数据库查询。

### 3.4 异步进化流水线 (Evolution Pipeline)
*   **Shadow Runner**: 在用户获取结果后，静默调用 LLM (DeepSeek/GPT-4) 生成其认为正确的语义路径。
*   **语义 Diff 算法**: 核心逻辑是将两者的逻辑路径标准化（Canonical Form）并对比 Hash。
*   **样本生成器**: 发现偏差时，自动格式化为 JSONL 格式，存入微调库。

## 4. 关键数据模型 (Rust Structs)
```rust
pub struct EntityMapping {
    pub id: uuid::Uuid,
    pub entity_uri: String,       // 本体唯一标识
    pub alias_list: Vec<String>,  // 人工标注别名
    pub physical_sql: String,     // 绑定的 SQL 片段
}

pub struct SessionContext {
    pub session_id: String,
    pub slots: Vec<IntentSlot>,   // 已识别槽位
    pub status: SessionStatus,    // 当前状态
    pub raw_query_log: Vec<String>, // 对话历史备份
}
```
