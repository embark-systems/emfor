## ADDED Requirements

### Requirement: Sticky uses weighted rendezvous hashing

For a service with `balance = "sticky"` and a non-empty affinity key, the client SHALL give each candidate a score and return the candidate with the largest score; on equal scores the candidate earlier in config order SHALL win. For a candidate with weight `w`, the score SHALL be the largest of the values `h_0` to `h_(w-1)`, where each value is the first 4 bytes of an MD5 digest read as a big-endian unsigned 32-bit integer: `h_0` is computed over the key bytes followed by the id bytes, and `h_i` for `i` from 1 is computed over the key bytes, the id bytes, the byte `#` and the decimal digits of `i`, all UTF-8 with no other separator. Each candidate SHALL therefore receive a share of keys equal to its weight divided by the sum of the candidates' weights. When every candidate has weight 1, the result SHALL be the candidate whose `MD5(key bytes followed by id bytes)` value is largest. A weight above 100, which the daemon never writes, SHALL count as 100, and a weight of 0 SHALL count as 1, so a damaged store cannot make one lookup compute more than 100 values per candidate. Slow start SHALL NOT change the weights used here. With no affinity key, or an empty one, the client SHALL pick by round-robin as defined above.

#### Scenario: Same key gives the same backend

- **WHEN** a fresh sticky service is looked up repeatedly with the same affinity key and its candidates and weights do not change
- **THEN** every lookup returns the same address

#### Scenario: Equal weights keep today's picks

- **WHEN** a fresh sticky service has backends `10.0.0.11:8080`, `10.0.0.12:8080`, `10.0.0.13:8080`, all with weight 1
- **THEN** the hash values and picks are as listed

##### Example: rendezvous hash vectors

| Key | Id | Hash (u32) |
| --- | --- | --- |
| `user-42` | `10.0.0.11:8080` | 1022706018 |
| `user-42` | `10.0.0.12:8080` | 3018330109 |
| `user-42` | `10.0.0.13:8080` | 336531056 |
| `user-7` | `10.0.0.11:8080` | 4189533513 |
| `user-7` | `10.0.0.12:8080` | 1529156490 |
| `user-7` | `10.0.0.13:8080` | 3484999621 |

##### Example: rendezvous picks

| Key | Healthy backends | Result |
| --- | --- | --- |
| `user-42` | all three | `10.0.0.12:8080` |
| `user-42` | `.11`, `.13` (`.12` unhealthy) | `10.0.0.11:8080` |
| `user-7` | all three | `10.0.0.11:8080` |
| `user-7` | `.12`, `.13` (`.11` unhealthy) | `10.0.0.13:8080` |

#### Scenario: Weights change the share of keys

- **WHEN** a fresh sticky service has backends `10.0.0.11:8080`, `10.0.0.12:8080` and `10.0.0.13:8080` with the weights listed, all healthy
- **THEN** the virtual node values and picks are as listed

##### Example: weighted hash vectors

| Key | Bytes hashed after the key | Hash (u32) |
| --- | --- | --- |
| `user-42` | `10.0.0.11:8080#1` | 101315482 |
| `user-42` | `10.0.0.11:8080#2` | 3080192789 |
| `user-42` | `10.0.0.13:8080#1` | 491054110 |
| `user-7` | `10.0.0.13:8080#1` | 2987525892 |

##### Example: weighted picks

| Key | Weights of `.11`, `.12`, `.13` | Result |
| --- | --- | --- |
| `user-42` | 1, 1, 1 | `10.0.0.12:8080` |
| `user-42` | 3, 1, 1 | `10.0.0.11:8080` |
| `user-42` | 1, 1, 100 | `10.0.0.13:8080` |
| `user-7` | 1, 1, 1 | `10.0.0.11:8080` |
| `user-7` | 2, 2, 2 | `10.0.0.11:8080` |
| `user-7` | 1, 1, 100 | `10.0.0.13:8080` |

#### Scenario: Raising a weight only moves keys to that backend

- **WHEN** the keys `key-0` to `key-999` are looked up with weights 1, 1, 1 and again with weights 1, 1, 2 for `.11`, `.12`, `.13`
- **THEN** with weights 1, 1, 2 `.13` gets 499 of the keys, and each of the 151 keys whose backend changed now goes to `.13`

#### Scenario: Weights above 100 count as 100

- **WHEN** a fresh sticky service has backends `10.0.0.11:8080` with weight 100 and `10.0.0.12:8080` with the weight listed, both healthy, and is looked up with the key listed
- **THEN** the result is as listed and the lookup returns as quickly as with weight 100

##### Example: weights above 100

| Key | Weight of `.12` | Result |
| --- | --- | --- |
| `key-795` | 100 | `10.0.0.11:8080` |
| `key-795` | 101 | `10.0.0.11:8080` |
| `key-795` | 4294967295 | `10.0.0.11:8080` |

With the weight of `.12` read as 101 the value of `h_100` (4277241900) would beat `.11` (4257715895) and `.12` would be picked.

#### Scenario: Empty key falls back to round-robin

- **WHEN** a fresh sticky service with three healthy backends is looked up 3 times with affinity key `""`
- **THEN** the 3 results are exactly the set of its three addresses

## REMOVED Requirements

### Requirement: Sticky uses rendezvous hashing version 1

**Reason**: Replaced by "Sticky uses weighted rendezvous hashing", which gives each candidate a share of keys in proportion to its weight. With every weight 1 it picks exactly as version 1 did.

**Migration**: Nothing to do for sticky services whose backends all have weight 1. A sticky service that already sets other weights moves some keys to other backends when its clients are upgraded; upgrade the clients of such a service together, because old and new clients pick differently for those keys.
