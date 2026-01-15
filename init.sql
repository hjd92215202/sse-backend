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


-- 1. 语义节点表 (维度/指标/物理表)
CREATE TABLE ontology_nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_key VARCHAR(100) UNIQUE NOT NULL,
    label VARCHAR(100) NOT NULL,
    node_type VARCHAR(20) NOT NULL CHECK (node_type IN ('METRIC', 'DIMENSION')),
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

-- 2. 语义定义与物理绑定 (前台配置核心)
CREATE TABLE semantic_definitions (
    node_id UUID REFERENCES ontology_nodes(id) ON DELETE CASCADE,
    source_id VARCHAR(50) NOT NULL,
    target_table VARCHAR(100) NOT NULL,
    target_column VARCHAR(100) NOT NULL,
    -- 核心：动态业务约束 (JSONB)
    -- 存储格式: [{"column": "status", "operator": "=", "value": "SUCCESS"}]
    default_constraints JSONB DEFAULT '[]',
    alias_names TEXT[] NOT NULL,
    PRIMARY KEY (node_id)
);

-- 3. 别名表 (用于 FST 索引)
CREATE TABLE node_aliases (
    id SERIAL PRIMARY KEY,
    node_id UUID REFERENCES ontology_nodes(id) ON DELETE CASCADE,
    alias_name VARCHAR(100) NOT NULL
);

-- 4. 本体关系表 (用于自动推理 JOIN 路径)
CREATE TABLE ontology_relations (
    id SERIAL PRIMARY KEY,
    from_node_id UUID REFERENCES ontology_nodes(id),
    to_node_id UUID REFERENCES ontology_nodes(id),
    join_logic TEXT NOT NULL -- 如: "a.platform_id = b.id"
);