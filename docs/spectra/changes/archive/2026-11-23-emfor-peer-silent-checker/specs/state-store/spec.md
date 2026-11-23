## ADDED Requirements

### Requirement: Silent checkers in the peer state

The key `meta/peers` SHALL hold, only while a member is silent, the field `silent_checkers`: an array of `{"service":<name>,"backend":<id>,"member":<member>}` for every silent checker (spec peer-group, "Silent assigned checkers"), ordered by service name, then backend id, then member. The daemon SHALL write the field only when the array is not empty, so that `meta/peers` is unchanged while no member is silent. A reader SHALL treat an absent field as an empty array.

#### Scenario: Silent member reaches the store

- **WHEN** a daemon in a peer group of three reports member `127.0.0.1:5304` silent for backend `127.0.0.1:8080` of `api.svc.emfor`
- **THEN** within 2 seconds `meta/peers` holds `"silent_checkers":[{"service":"api.svc.emfor","backend":"127.0.0.1:8080","member":"127.0.0.1:5304"}]`

#### Scenario: Field absent while nobody is silent

- **WHEN** no member is silent
- **THEN** `meta/peers` has no `silent_checkers` key
