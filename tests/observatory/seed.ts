import { SQL } from "bun";

export type Profile = "correctness" | "small" | "medium" | "large";

const profileRows: Record<Profile, number> = {
  correctness: 12,
  small: 10_000,
  medium: 250_000,
  large: 1_000_000,
};

export async function seed(sql: SQL, profile: Profile): Promise<void> {
  const rows = profileRows[profile];
  await sql.begin(async (transaction) => {
    await transaction`INSERT INTO tenants (id, name) VALUES
      ('018f6f19-795f-7c3d-b1b3-8f177ab8a301', 'Northern Array'),
      ('018f6f19-795f-7c3d-b1b3-8f177ab8a302', 'Southern Array')`;
    await transaction`INSERT INTO networks (tenant_id, code, name) VALUES
      ('018f6f19-795f-7c3d-b1b3-8f177ab8a301', 'aurora', 'Aurora Network'),
      ('018f6f19-795f-7c3d-b1b3-8f177ab8a302', 'horizon', 'Horizon Network')`;
    await transaction`INSERT INTO stations
      (tenant_id, network_code, code, name, latitude, longitude, commissioned_at, metadata)
      VALUES
      ('018f6f19-795f-7c3d-b1b3-8f177ab8a301', 'aurora', 'north-1', 'North One', 69.6492, 18.9553, '2020-01-01T00:00:00Z', '{"terrain":"coastal"}'),
      ('018f6f19-795f-7c3d-b1b3-8f177ab8a302', 'horizon', 'south-1', 'South One', -33.8688, 151.2093, '2021-06-01T00:00:00Z', '{"terrain":"urban"}')`;
    await transaction`INSERT INTO sensors
      (tenant_id, network_code, station_code, code, unit) VALUES
      ('018f6f19-795f-7c3d-b1b3-8f177ab8a301', 'aurora', 'north-1', 'temperature', 'celsius'),
      ('018f6f19-795f-7c3d-b1b3-8f177ab8a301', 'aurora', 'north-1', 'humidity', 'percent'),
      ('018f6f19-795f-7c3d-b1b3-8f177ab8a302', 'horizon', 'south-1', 'temperature', 'celsius')`;
    await transaction`INSERT INTO readings
      (tenant_id, network_code, station_code, sensor_code, recorded_at, value, confidence, flagged, payload)
      SELECT
        CASE WHEN sample % 2 = 0 THEN '018f6f19-795f-7c3d-b1b3-8f177ab8a301'::uuid ELSE '018f6f19-795f-7c3d-b1b3-8f177ab8a302'::uuid END,
        CASE WHEN sample % 2 = 0 THEN 'aurora' ELSE 'horizon' END,
        CASE WHEN sample % 2 = 0 THEN 'north-1' ELSE 'south-1' END,
        'temperature',
        '2024-01-01T00:00:00Z'::timestamptz + sample * interval '1 minute',
        ((sample % 700) - 200)::numeric / 10,
        CASE WHEN sample % 6 = 0 THEN NULL ELSE (sample % 100)::double precision / 100 END,
        sample % 4 = 0,
        jsonb_build_object('sequence', sample)
      FROM generate_series(1, ${rows}) AS sample`;
    await transaction`INSERT INTO wide_integer_values (value)
      VALUES (9007199254740993)`;
    await transaction`INSERT INTO structured_type_values
      (id, label, address, labels, wide_values, wide_matrix, addresses, sensor_rows)
      VALUES (
        1,
        'primary',
        '127.0.0.1',
        ARRAY['north', 'south'],
        ARRAY[9007199254740993, 2],
        ARRAY[[9007199254740993, 2], [3, 4]],
        ARRAY['127.0.0.1'::inet, '2001:db8::1'::inet],
        ARRAY[
          ROW(
            '018f6f19-795f-7c3d-b1b3-8f177ab8a301'::uuid,
            'aurora',
            'north-1',
            'temperature',
            'celsius'
          )::sensors
        ]
      )`;
    await transaction`REFRESH MATERIALIZED VIEW network_reading_totals`;
  });
}
