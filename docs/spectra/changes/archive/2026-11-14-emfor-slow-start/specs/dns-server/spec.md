## ADDED Requirements

### Requirement: SRV weights during slow start

When at least one SRV candidate is ramping, as defined by the backend-selection requirement "Slow start effective weight", each SRV record's weight SHALL be that candidate's effective weight, for every `balance` value except `failover`; priorities and record order SHALL follow the requirement "SRV answers" unchanged. When no candidate is ramping, or for `failover`, weights SHALL be as in "SRV answers".

#### Scenario: Round-robin with a ramping backend

- **WHEN** a `round-robin` service `api.svc.emfor` has healthy backends `10.0.0.11:8080` (not ramping) and `10.0.0.12:8080` (step 3), and a client queries SRV with a fresh SRV counter
- **THEN** the answer is `0 10 8080 10-0-0-11.api.svc.emfor.` followed by `0 3 8080 10-0-0-12.api.svc.emfor.`

#### Scenario: Weighted with a ramping backend

- **WHEN** a `weighted` service has healthy backends `10.0.0.11:8080` (weight 3, step 2) and `10.0.0.12:8080` (weight 1, not ramping)
- **THEN** the SRV weights are 6 and 10 in config order

#### Scenario: Failover keeps weight 1

- **WHEN** a `failover` service has healthy backends `10.0.0.11:8080` (step 2) and `10.0.0.12:8080` (not ramping)
- **THEN** the answer is `0 1 8080 10-0-0-11.api.svc.emfor.` followed by `1 1 8080 10-0-0-12.api.svc.emfor.`
