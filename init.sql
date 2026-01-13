CREATE TABLE semantic_mappings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_key VARCHAR(100) NOT NULL UNIQUE,  -- 语义 ID: profit_platform
    entity_label VARCHAR(100) NOT NULL,       -- 中文名: 分润平台
    alias_names TEXT[] NOT NULL,              -- 别名: ["分润", "平台"]
    target_table VARCHAR(100) NOT NULL,       -- 物理表: t_revenue
    target_column VARCHAR(100) NOT NULL,      -- 物理列: platform_name
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

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

-- 2. 修改映射表，关联数据源
ALTER TABLE semantic_mappings ADD COLUMN source_id VARCHAR(50) REFERENCES data_sources(id);