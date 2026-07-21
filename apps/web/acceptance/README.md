# Human acceptance environment

Run the persistent candidate environment through the package script. The command prints the exact
candidate manifest and stays alive until interrupted. Formal real-phone runs must provide a
reachable non-loopback `--listen` address.

H1 and H2 use the instrumented embedded build:

```bash
npm run acceptance:human -- --gate h1-h2 --listen "$LAN_IP:0"
```

H3 uses the executable extracted from the selected release archive:

```bash
npm run acceptance:human -- --gate h3 --artifact "$A5_ARCHIVE" \
  --candidate-manifest "$A5_MANIFEST" --listen "$LAN_IP:0"
```

`A5_MANIFEST` is the `a5-manifest.json` emitted by the same release-candidate run. The launcher
rejects archives not listed by that manifest and carries forward its promotion asset identity.

The command creates no decision. A human reviewer records evidence separately against
`evidence.schema.json`; never add reviewer evidence to the repository.
