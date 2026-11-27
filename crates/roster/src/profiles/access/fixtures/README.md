# Access fixtures

Replies of Agent Backbone's access routes, in the shapes its protocol crate
documents: the access fields of `GET /api/agents` (`agents.json`),
`GET /api/swarms` (`swarms.json`), `GET /api/access/requests` pending and
all (`requests-*.json`), a change's reply (`agent-access.json`), a placement
(`agent-placed.json`), a decided request (`request-approved.json`), and one
refusal per error code (`refusal-*.json`, each the body Backbone sends with
403, 404, 409 or 422). Names and repositories are examples.

`agents.json` holds one grant of a kind this version does not know
(`calendar`), a session Backbone lists without registering it (`scratch`) and
one it does not manage (`stray`).
