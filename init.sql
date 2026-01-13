CREATE TABLE semantic_mappings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_key VARCHAR(100) NOT NULL UNIQUE,  -- 语义 ID: profit_platform
    entity_label VARCHAR(100) NOT NULL,       -- 中文名: 分润平台
    alias_names TEXT[] NOT NULL,              -- 别名: ["分润", "平台"]
    target_table VARCHAR(100) NOT NULL,       -- 物理表: t_revenue
    target_column VARCHAR(100) NOT NULL,      -- 物理列: platform_name
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);