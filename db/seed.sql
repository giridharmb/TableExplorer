-- Mock data: two tables with different ("mixed") schemas, 1000 rows each,
-- with deliberate NULLs and dirty values to exercise the search UI.
DROP TABLE IF EXISTS customers;
DROP TABLE IF EXISTS sensor_readings;

CREATE TABLE customers (
    id          serial PRIMARY KEY,
    uid         uuid,
    name        text,
    email       text,
    age         int,
    balance     numeric(12,2),
    active      boolean,
    country     varchar(40),
    tags        text[],
    profile     jsonb,
    signup_at   timestamptz,
    notes       text
);

INSERT INTO customers (uid, name, email, age, balance, active, country, tags, profile, signup_at, notes)
SELECT
    CASE WHEN g % 37 = 0 THEN NULL ELSE md5(g::text)::uuid END,
    CASE WHEN g % 53 = 0 THEN NULL
         ELSE (ARRAY['Alice','Bob','Carol','Dave','Eve','Frank','Grace','Heidi','Ivan','Judy','Mallory','Oscar'])[1 + g % 12]
              || ' ' || (ARRAY['Smith','Johnson','Lee','Garcia','Brown','Nguyen','Patel','Kim','Müller','O''Brien'])[1 + (g / 12) % 10]
    END,
    CASE WHEN g % 41 = 0 THEN 'not-an-email'
         WHEN g % 29 = 0 THEN NULL
         ELSE lower((ARRAY['alice','bob','carol','dave','eve','frank','grace','heidi','ivan','judy','mallory','oscar'])[1 + g % 12])
              || g || '@' || (ARRAY['example.com','mail.org','corp.net','test.io'])[1 + g % 4]
    END,
    CASE WHEN g % 31 = 0 THEN NULL WHEN g % 97 = 0 THEN -1 ELSE 18 + (g * 7) % 60 END,
    CASE WHEN g % 23 = 0 THEN NULL WHEN g % 89 = 0 THEN 'NaN'::numeric ELSE round(((g * 7919) % 100000) / 7.0, 2) END,
    CASE WHEN g % 19 = 0 THEN NULL ELSE g % 3 <> 0 END,
    CASE WHEN g % 43 = 0 THEN '' WHEN g % 47 = 0 THEN NULL
         ELSE (ARRAY['USA','India','Germany','Brazil','Japan','Canada','France','Kenya'])[1 + g % 8] END,
    CASE WHEN g % 17 = 0 THEN NULL WHEN g % 13 = 0 THEN '{}'
         ELSE ARRAY[(ARRAY['vip','new','churn-risk','beta','partner'])[1 + g % 5], (ARRAY['eu','us','apac'])[1 + g % 3]] END,
    CASE WHEN g % 11 = 0 THEN NULL
         WHEN g % 61 = 0 THEN '{"error": "profile sync failed", "code": 500}'::jsonb
         ELSE jsonb_build_object('plan', (ARRAY['free','pro','enterprise'])[1 + g % 3],
                                 'logins', (g * 13) % 500,
                                 'prefs', jsonb_build_object('theme', CASE WHEN g % 2 = 0 THEN 'dark' ELSE 'light' END)) END,
    CASE WHEN g % 27 = 0 THEN NULL ELSE timestamptz '2020-01-01' + (g * interval '37 hours') END,
    CASE WHEN g % 5 = 0 THEN NULL
         WHEN g % 33 = 0 THEN '#ERROR! value could not be parsed'
         WHEN g % 7 = 0 THEN 'Order #' || (10000 + g) || ' shipped via ' || (ARRAY['UPS','FedEx','DHL'])[1 + g % 3]
         ELSE 'Ticket TK-' || lpad((g * 3)::text, 5, '0') || ': ' || (ARRAY['login issue','billing question','feature request','bug report'])[1 + g % 4] END
FROM generate_series(1, 1000) AS g;

CREATE TABLE sensor_readings (
    reading_id  bigserial PRIMARY KEY,
    device      text NOT NULL,
    location    point,
    temperature double precision,
    humidity    real,
    status      text,
    recorded_on date,
    raw_payload bytea,
    meta        json
);

INSERT INTO sensor_readings (device, location, temperature, humidity, status, recorded_on, raw_payload, meta)
SELECT
    'dev-' || lpad((g % 50)::text, 3, '0'),
    CASE WHEN g % 9 = 0 THEN NULL ELSE point((g % 180) - 90, (g * 3 % 360) - 180) END,
    CASE WHEN g % 21 = 0 THEN NULL WHEN g % 77 = 0 THEN 'Infinity'::float8 WHEN g % 83 = 0 THEN 'NaN'::float8
         ELSE round(((g * 37) % 600) / 10.0 - 10, 1) END,
    CASE WHEN g % 14 = 0 THEN NULL ELSE ((g * 17) % 1000) / 10.0 END,
    CASE WHEN g % 25 = 0 THEN 'ERR_TIMEOUT' WHEN g % 18 = 0 THEN NULL
         ELSE (ARRAY['ok','ok','ok','warning','critical'])[1 + g % 5] END,
    CASE WHEN g % 16 = 0 THEN NULL ELSE date '2024-01-01' + g END,
    CASE WHEN g % 4 = 0 THEN NULL ELSE convert_to('pkt' || g, 'UTF8') END,
    CASE WHEN g % 10 = 0 THEN NULL ELSE json_build_object('fw', '1.' || (g % 9), 'battery', (g * 7) % 100) END
FROM generate_series(1, 1000) AS g;
