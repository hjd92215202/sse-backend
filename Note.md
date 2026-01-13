--pg
CREATE TABLE t_finance_detail (
    id SERIAL PRIMARY KEY,
    platform_name VARCHAR(100),
    amount NUMERIC(15, 2),
    dt DATE
);
INSERT INTO t_finance_detail (platform_name, amount, dt) VALUES ('A公司', 5000.00, '2025-01-01');
-mysql

CREATE TABLE t_operation_revenue (
    id INT AUTO_INCREMENT PRIMARY KEY,
    corp_name VARCHAR(100),
    income DECIMAL(15, 2),
    created_at DATETIME
);
INSERT INTO t_operation_revenue (corp_name, income, created_at) VALUES ('B公司', 8888.88, NOW());

通过 API 注册数据源与映射

curl -X POST http://localhost:3000/api/datasource \
-H "Content-Type: application/json" \
-d '{
  "id": "pg_source",
  "db_type": "postgres",
  "connection_url": "postgres://postgres:123@localhost:5432/sse_db",
  "display_name": "财务部PG库"
}'


curl -X POST http://localhost:3000/api/datasource \
-H "Content-Type: application/json" \
-d '{
  "id": "mysql_source",
  "db_type": "mysql",
  "connection_url": "mysql://root:root@localhost:3306/indicator_cls",
  "display_name": "运营部MySQL库"
}'


curl -X POST http://localhost:3000/api/mapping \
-H "Content-Type: application/json" \
-d '{
  "entity_key": "finance_platform",
  "entity_label": "结算平台",
  "alias_names": ["结算库"],
  "target_table": "t_finance_detail",
  "target_column": "platform_name",
  "source_id": "pg_source"
}'


curl -X POST http://localhost:3000/api/mapping \
-H "Content-Type: application/json" \
-d '{
  "entity_key": "op_platform",
  "entity_label": "运营平台",
  "alias_names": ["运营库"],
  "target_table": "t_operation_revenue",
  "target_column": "corp_name",
  "source_id": "mysql_source"
}'