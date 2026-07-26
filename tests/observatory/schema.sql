DROP SCHEMA public CASCADE;
CREATE SCHEMA public;
ALTER DATABASE observatory SET timezone = 'UTC';
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE tenants (
  id uuid PRIMARY KEY,
  name text NOT NULL UNIQUE
);

CREATE TABLE networks (
  tenant_id uuid NOT NULL REFERENCES tenants(id),
  code text NOT NULL,
  name text NOT NULL,
  active boolean NOT NULL DEFAULT true,
  PRIMARY KEY (tenant_id, code)
);

CREATE TABLE stations (
  tenant_id uuid NOT NULL,
  network_code text NOT NULL,
  code text NOT NULL,
  name text NOT NULL,
  latitude numeric NOT NULL,
  longitude numeric NOT NULL,
  commissioned_at timestamptz NOT NULL,
  metadata jsonb NOT NULL DEFAULT '{}',
  PRIMARY KEY (tenant_id, network_code, code),
  FOREIGN KEY (tenant_id, network_code)
    REFERENCES networks(tenant_id, code)
);

CREATE TABLE sensors (
  tenant_id uuid NOT NULL,
  network_code text NOT NULL,
  station_code text NOT NULL,
  code text NOT NULL,
  unit text NOT NULL,
  PRIMARY KEY (tenant_id, network_code, station_code, code),
  FOREIGN KEY (tenant_id, network_code, station_code)
    REFERENCES stations(tenant_id, network_code, code)
);

CREATE TABLE readings (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  tenant_id uuid NOT NULL,
  network_code text NOT NULL,
  station_code text NOT NULL,
  sensor_code text NOT NULL,
  recorded_at timestamptz NOT NULL,
  value numeric NOT NULL,
  confidence double precision,
  flagged boolean NOT NULL DEFAULT false,
  payload jsonb NOT NULL DEFAULT '{}',
  FOREIGN KEY (tenant_id, network_code, station_code, sensor_code)
    REFERENCES sensors(tenant_id, network_code, station_code, code)
);

CREATE INDEX readings_sensor_time_idx ON readings
  (tenant_id, network_code, station_code, sensor_code, recorded_at DESC)
  INCLUDE (value);

CREATE INDEX sensors_code_search_idx ON sensors
  USING gin (code gin_trgm_ops);

CREATE VIEW active_stations AS
SELECT tenant_id, network_code, code, name
FROM stations;

CREATE MATERIALIZED VIEW network_reading_totals AS
SELECT tenant_id, network_code, count(*) AS reading_count
FROM readings
GROUP BY tenant_id, network_code;

COMMENT ON TABLE networks IS 'Named observation networks within a tenant.';
COMMENT ON COLUMN networks.code IS 'Stable network code within its tenant.';
COMMENT ON TABLE stations IS 'Physical stations that collect observations.';
COMMENT ON COLUMN stations.metadata IS 'Provider-specific station attributes.';
COMMENT ON TABLE readings IS 'Time-series values emitted by station sensors.';
COMMENT ON COLUMN readings.confidence IS 'Optional confidence score from zero to one.';
COMMENT ON VIEW active_stations IS 'Stations currently exposed to readers.';
COMMENT ON COLUMN active_stations.code IS 'Station code within the composite network key.';
COMMENT ON MATERIALIZED VIEW network_reading_totals IS 'Precomputed reading counts per network.';
