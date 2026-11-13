# Sites Worker

The core allocator (`arbos_core::sites`) defines valid user and app names. The
Worker mirrors those naming rules for the deployed subdomain and two-level
schemes. `tests/routes.json` is the shared contract: its reserved names and route
vectors run against both the actual core parser and the actual Worker resolver.

Run the local checks without installing Wrangler or contacting R2:

```sh
node --test edge/sites-worker/tests/routes.test.js
cargo test --locked -p arbos-core --test sites_routes
```

The Worker also has serving-specific responses: user landing pages, redirects
for missing final slashes, and invalid-app pages. Its path scheme passes through
because this Worker does not front the apex. The Node tests retain these
distinctions while comparing resolved site fields with core.
