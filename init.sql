-- 创建测试物理表
CREATE TABLE t_revenue_data (
    id SERIAL PRIMARY KEY,
    platform_name VARCHAR(100),
    revenue NUMERIC(15, 2),
    report_date DATE
);

-- 插入模拟数据
INSERT INTO t_revenue_data (platform_name, revenue, report_date) VALUES 
('A公司', 12500.00, '2025-12-24'),
('B公司', 8800.50, '2025-12-24'),
('A公司', 3100.00, '2025-12-25');

CREATE TABLE data_sources (
    id VARCHAR(50) PRIMARY KEY,       -- 唯一标识，如 "finance_db"
    db_type VARCHAR(20) NOT NULL,     -- postgres, mysql
    connection_url TEXT NOT NULL,     -- 加密存储或直接存储连接串
    display_name VARCHAR(100),
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);


-- 1. 本体节点：定义核心概念
CREATE TABLE ontology_nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_key VARCHAR(100) UNIQUE NOT NULL, 
    label VARCHAR(100) NOT NULL,
    node_role VARCHAR(20) NOT NULL CHECK (node_role IN ('METRIC', 'DIMENSION')),
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

-- 2. 语义定义与物理绑定
CREATE TABLE semantic_definitions (
    node_id UUID PRIMARY KEY REFERENCES ontology_nodes(id) ON DELETE CASCADE,
    source_id VARCHAR(50) NOT NULL,
    target_table VARCHAR(100) NOT NULL,
    target_column VARCHAR(100) NOT NULL,
    default_constraints JSONB DEFAULT '[]', -- 指标的隐含过滤逻辑
    alias_names TEXT[] NOT NULL
);

-- 增加默认聚合方式字段
ALTER TABLE semantic_definitions ADD COLUMN default_agg VARCHAR(20) DEFAULT 'SUM';

-- 3. RDF 语义关联表 (T-Box)
-- 定义某个指标支持哪些维度的切片推理
CREATE TABLE metric_dimension_rels (
    metric_node_id UUID REFERENCES ontology_nodes(id) ON DELETE CASCADE,
    dimension_node_id UUID REFERENCES ontology_nodes(id) ON DELETE CASCADE,
    PRIMARY KEY (metric_node_id, dimension_node_id)
);

-- 4. 维度实例值存储 (A-Box)
-- 自动从业务库同步，如：维度'业务模式' -> 值'分润平台'
CREATE TABLE dimension_values (
    id SERIAL PRIMARY KEY,
    dimension_node_id UUID REFERENCES ontology_nodes(id) ON DELETE CASCADE,
    value_label VARCHAR(200) NOT NULL, -- 用户输入的文本: "分润平台"
    value_code VARCHAR(200) NOT NULL,  -- 数据库存储的码值: "PLAT_01"
    UNIQUE(dimension_node_id, value_label)
);

-- 5. 多表关联路径 (预留给后续自动 JOIN 推理)
CREATE TABLE ontology_relations (
    id SERIAL PRIMARY KEY,
    from_table VARCHAR(100),
    to_table VARCHAR(100),
    join_logic TEXT NOT NULL
);