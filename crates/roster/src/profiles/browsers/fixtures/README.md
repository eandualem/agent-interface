# Browser fixtures

Agent Backbone's named-browser contract, copied from its protocol crate's
fixtures: the replies of `GET /api/browsers` (`browsers.json`),
`GET /api/browsers/{id}` and the routes that change a browser
(`browser.json`) and `GET /api/browsers/{id}/sessions`
(`browser-sessions.json`); the bodies of `POST /api/browsers`
(`browser-create.json`), `.../save` (`browser-save.json`) and
`PUT .../policies` (`browser-policies.json`); the push stream's
`browser.changed` event (`browser-changed*.json`); a grant naming a browser
(`agent-grant-browser.json`) and an agent's request for one
(`access-request-browser.json`). Names are examples.
