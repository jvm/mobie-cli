# MOBIE API Reference

## Base URL

- Production base URL: `https://pgm.mobie.pt`
- API prefix: `/api`

## Response Envelope

Known endpoints return a JSON envelope with this shape:

```json
{
  "data": {},
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2025-01-01T00:00:00Z"
}
```

Fields:

- `data`: endpoint payload
- `status_code`: numeric status indicator
- `status_message`: human-readable status text
- `timestamp`: server timestamp

## Authentication

### Login

`POST /api/login`

Request body:

```json
{
  "email": "user@example.com",
  "password": "secret"
}
```

Response body:

```json
{
  "data": {
    "bearer": {
      "access_token": "token",
      "refresh_token": "refresh-token",
      "expires_in": 3600,
      "refresh_expires_in": 1800,
      "token_type": "Bearer",
      "id_token": "id-token",
      "not-before-policy": 0,
      "session_state": "session-id",
      "scope": "scope"
    },
    "user": {
      "email": "user@example.com",
      "entity": "entity",
      "roles": [
        {
          "profile": "DPC",
          "role": "OPERATOR",
          "name": "Operator"
        }
      ]
    }
  }
}
```

### Refresh Token

`POST /api/refresh`

Request body:

```json
{
  "refresh_token": "refresh-token"
}
```

Response body:

- Same shape as `POST /api/login`

### Authenticated Request Headers

Authenticated requests use:

- `authorization: Bearer <access_token>`
- `user: <user.email>`
- `profile: <profile>` on endpoints that require a profile context

Notes:

- `profile` is commonly sent on authenticated backoffice requests.
- Some authenticated endpoints have been observed to succeed with `authorization` and `user` only.

## Endpoints

### List Locations

`GET /api/locations?limit=0&offset=0`

Response example:

```json
{
  "data": [
    {
      "id": "MOBI-XXX-00000",
      "name": "Station Alpha"
    },
    {
      "id": "MOBI-LSB-00694",
      "name": "Station Beta"
    }
  ],
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2025-10-10T12:00:00Z"
}
```

Observed item fields:

- `id`
- `name`
- other fields may be present

### Get Location

`GET /api/locations/{locationId}`

Response example:

```json
{
  "data": {
    "id": "MOBI-AAA-00001",
    "status": "ACTIVE"
  },
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2025-01-01T00:00:00Z"
}
```

Observed item fields:

- `id`
- `status`
- other fields may be present

Additional observed fields in full location detail responses include:

- `party_id`
- `country_code`
- `address`
- `postal_code`
- `country`
- `time_zone`
- `mobie_cpe`
- `operator`
- `suboperator`
- `opening_times`
- `evses`
- `facilities`
- `energy_mix`
- multiple `pdgr*` operational fields

Example nested fields observed under `evses[].connectors[]`:

- `standard`
- `format`
- `power_type`
- `max_voltage`
- `max_amperage`
- `max_electric_power`
- `tariff_ids`
- `terms_and_conditions`
- `pdgrConnectorStatus`

### Location Analytics

`GET /api/locations/analytics`

Response example:

```json
{
  "data": {
    "locationsTotalCount": 1,
    "evsesTotalCount": 1,
    "connectorsTotalCount": 1,
    "locsInUseTotalCount": 1,
    "evsesInUseTotalCount": 1,
    "locationsByConnectivity": [
      { "_id": "ONLINE", "count": 1 }
    ],
    "evsesByStatus": [
      { "_id": "CHARGING", "count": 1 }
    ]
  },
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2026-03-05T23:20:23.042Z"
}
```

### Locations GeoJSON

`GET /api/locations/geojson`

Behavior:

- Returns a large JSON payload used to render the map view.
- The response body was too large to inspect inline, but the route is confirmed and returns `200`.
- Content type is `application/json; charset=utf-8`.

### List Sessions

`GET /api/sessions`

Query parameters:

- `limit=<n>`
- `offset=<n>`
- `locationId=<location id>`
- `dateFrom=<ISO-8601 UTC timestamp>` optional
- `dateTo=<ISO-8601 UTC timestamp>` optional

Example:

```text
/api/sessions?limit=10&offset=0&locationId=EVSE-1&dateFrom=2025-01-02T00:00:00.000Z&dateTo=2025-01-02T23:59:59.999Z
```

Response example:

```json
{
  "data": [
    {
      "id": "sess-1",
      "start_date_time": "2025-10-10T10:00:00Z",
      "end_date_time": "2025-10-10T11:00:00Z",
      "kwh": 5.0,
      "status": "COMPLETED",
      "location_id": "EVSE-1",
      "evse_uid": "EVSE-UID-1",
      "connector_id": "1",
      "pdgrTransactionId": 123,
      "cdr_token": {
        "uid": "token-1",
        "type": "RFID",
        "contract_id": "C-1",
        "pdgrPartyId": "PTABC",
        "pdgrVisualNumber": "123456789"
      },
      "charging_periods": [
        {
          "start_date_time": "2025-10-10T10:00:00Z",
          "dimensions": [
            {
              "type": "ENERGY",
              "volume": 1.0
            }
          ],
          "tariff_id": "tariff-1"
        }
      ]
    }
  ],
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2025-10-10T12:00:00Z"
}
```

Observed session fields:

- `id`
- `start_date_time`
- `end_date_time`
- `kwh`
- `status`
- `location_id`
- `evse_uid`
- `connector_id`
- `pdgrTransactionId`
- `cdr_token`
- `charging_periods`
- other fields may be present

Notes:

- `charging_periods[].dimensions[].volume` may be numeric or string.
- `cdr_token.type` is returned as `type`.
- the API may return sessions that overlap the requested date window, not only sessions whose `start_date_time` falls fully inside it
- `mobie` preserves that overlap behavior in its canonical local session query path
- cached session rows are keyed locally by the API `id`

### List Tokens

`GET /api/tokens`

Query parameters:

- `limit=<n>`
- `offset=<n>`

Response example:

```json
{
  "data": [
    {
      "uid": "token-1"
    }
  ],
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2025-01-01T00:00:00Z"
}
```

Observed item fields:

- `uid`
- `token_uid` may also appear
- other fields may be present

### List OCPP Logs

`GET /api/logs/ocpp`

Query parameters:

- `limit=<n>`
- `offset=<n>`
- `error=true` optional

Response example:

```json
{
  "data": [
    {
      "id": "log-1",
      "messageType": "MeterValues",
      "direction": "Request",
      "timestamp": "2025-10-10T10:05:00Z",
      "logs": "{\"transactionId\":123,\"meterValue\":[]}"
    }
  ],
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2025-10-10T12:00:00Z"
}
```

Observed item fields:

- `id`
- `messageType`
- `direction`
- `timestamp`
- `logs`
- other fields may be present

Notes:

- `logs` may be a stringified JSON payload or a JSON value.
- the API `id` appears to identify the charger/location, not a unique log entry
- `mobie` therefore uses a synthetic local fingerprint for canonical log identity
- local ordered reads use `timestamp` plus a deterministic sort key derived during ingestion
- `logs list --error-only` is tracked separately in cache freshness scope from unfiltered OCPP log reads

### List OCPI Logs

`GET /api/logs/ocpi`

Query parameters:

- `limit=<n>`
- `offset=<n>`

Notes:

- The route exists.
- Access is permission-gated and may return `401` for profiles without OCPI log access.

### Get Entity

`GET /api/entities/{entityCode}`

Response example:

```json
{
  "data": {
    "name": "Entity Name",
    "code": "0315",
    "countryCode": "PT",
    "fiscalNumber": "218598351",
    "vatCode": 23,
    "emsp": false,
    "cpo": false,
    "dpc": true,
    "cse": false,
    "street": "Street",
    "city": "Lisboa",
    "zip": "1750-004",
    "disabled": false,
    "partyIds": [],
    "mobieNetworkConnection": true,
    "tokenRepresentation": "DECIMAL"
  },
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2026-03-05T23:20:22.231Z"
}
```

Observed item fields:

- `name`
- `code`
- `countryCode`
- `fiscalNumber`
- `vatCode`
- boolean flags such as `emsp`, `cpo`, `dpc`, `cse`
- address and support-contact fields
- frontend capability flags such as `frontendDpc`
- `partyIds`
- `mobieNetworkConnection`
- `tokenRepresentation`

### Get Role

`GET /api/identity/roles/{roleName}`

Response example:

```json
{
  "data": {
    "profile": "DPC",
    "role": "Operator",
    "name": "DPC_OPERATOR",
    "frontend": true,
    "modules": [
      {
        "name": "Locations Manager",
        "read": true,
        "write": false,
        "delete": false,
        "operate": false
      }
    ]
  },
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2026-03-05T23:20:22.844Z"
}
```

Observed item fields:

- `profile`
- `role`
- `name`
- `frontend`
- `modules[]`

Observed module permission fields:

- `name`
- `read`
- `write`
- `delete`
- `operate`

### List ORDs

`GET /api/ords`

Response example:

```json
{
  "data": [
    {
      "cpe": "PT0002000066000688BY",
      "cpeStatus": "Integrated",
      "requestDate": "2023-11-16T17:02:38.927Z",
      "location_id": "MOBI-LSB-00693",
      "area": "PT",
      "entityCode": "MOBI",
      "tax_number": "509767606",
      "voltage_level": "BTN",
      "integrationDate": "2023-11-21T04:31:57.326Z",
      "location_active": true,
      "mobie_network": true
    }
  ],
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2026-03-05T23:20:21.808Z"
}
```

Observed item fields:

- `cpe`
- `cpeStatus`
- `requestDate`
- `integrationDate`
- `location_id`
- `area`
- `entityCode`
- `tax_number`
- `voltage_level`
- `location_active`
- `mobie_network`

### ORD Statistics

`GET /api/ords/statistics`

Response example:

```json
{
  "data": {
    "cpesTotalCount": 1,
    "cpesCount": {
      "cpeStatusIntegrated": 1,
      "cpeStatusNotIntegrated": 0
    }
  },
  "status_code": 1000,
  "status_message": "Success",
  "timestamp": "2026-03-05T23:20:22.035Z"
}
```

### ORD Integrated/Queued Lists

`GET /api/ords/cpesIntegrated`

`GET /api/ords/cpesToIntegrate`

Behavior:

- Both routes are confirmed.
- Both return the standard envelope with `data` as an array.
- Empty arrays were observed for the authenticated account used during validation.

### Additional Observed ORD Routes

The following ORD-related routes are present but may be permission-gated or data-dependent:

- `GET /api/ords/analytics`
- `GET /api/ords/totalCPEConsumptionDay`
- `GET /api/ords/totalCPEConsumptionMonth`
- `GET /api/ords/totalORDConsumptionDay`
- `GET /api/ords/totalORDConsumptionMonth`

Observed behavior:

- `/api/ords/analytics` returned `404` with `ResourceNotFoundError: No CPE found`.
- The `total*Consumption*` routes returned `401` for the validated profile.

## Pagination

Collection endpoints use offset pagination:

- Start with `offset=0`
- Increase `offset` by the number of items returned
- Stop when `data` is an empty array

Observed collection endpoints using this pattern:

- `/api/sessions`
- `/api/tokens`
- `/api/logs/ocpp`
- `/api/logs/ocpi`

Additional observed header:

- `x-total-count` may be present on list responses. It was observed on `GET /api/locations?limit=0&offset=0`.

## Error Semantics

Observed HTTP status handling:

- `401` or `403`: unauthorized
- `429`: rate limited
- `500` to `599`: server error
- other non-2xx statuses: request failure

Observed application-level error payloads also use the standard envelope metadata fields and may return messages such as:

- `UnauthorizedError: Authorization header - Cannot perform this operation`
- `ResourceNotFoundError: No CPE found`

## Permission Model

Access to routes is role-dependent.

Observed role permissions are exposed by `GET /api/identity/roles/{roleName}` as a per-module capability matrix with:

- `read`
- `write`
- `delete`
- `operate`

As a result:

- A route may exist but still return `401` for a given authenticated profile.
- Different roles may expose materially different readable subsets of the API.
