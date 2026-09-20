# Deploying

`demo.ovk.zero2one.ee` is served by the `ovk-demo` LXC container on the Hetzner
box (46.4.94.200). The host's Caddy terminates TLS and reverse-proxies to
`10.14.235.94:8787`; inside the container the server runs as the systemd unit
`ovk-demo.service` out of `/opt/ovk-prototype`.

Two ways to deploy, both running the same script.

## From the Actions tab (no credentials needed)

Push to `main`, or open **Actions → Deploy → Run workflow**. That is the whole
procedure. You never need an SSH key, the server address, or access to anyone's
machine — the key is a repository secret, readable only by the runner.

**Who this covers: anyone with write access** to the repository holding the
secret. Read access is not enough and is not meant to be — this repository is
public, so a clone grants no route to the server, and neither does a pull
request from a fork (GitHub withholds secrets from those by design). Deploying
is therefore exactly as restricted as pushing.

## From a clone

```sh
./deploy/deploy.sh
```

Needs SSH access to the host as a user with passwordless `sudo`. It uses
whatever key your agent holds, or:

```sh
DEPLOY_SSH_KEY_FILE=~/.ssh/ovk_deploy ./deploy/deploy.sh
```

Everything is overridable, so the same script deploys to a different box or
container without editing it:

| Variable | Default |
|---|---|
| `DEPLOY_HOST` | `46.4.94.200` |
| `DEPLOY_USER` | `root` |
| `DEPLOY_CONTAINER` | `ovk-demo` |
| `DEPLOY_PATH` | `/opt/ovk-prototype` |
| `DEPLOY_SERVICE` | `ovk-demo` |
| `DEPLOY_HEALTH_URL` | `https://demo.ovk.zero2one.ee/mockup` |
| `DEPLOY_REF` | `HEAD` |

## What it does

1. `git archive` of the committed tree — what ships is exactly what is in git.
   Local edits are never deployed, and `.git` never reaches the server.
2. Preflight: `lxc` present, passwordless `sudo`, container `RUNNING`.
3. Copies the tarball to the host, then `lxc file push` into the container.
   There is no SSH endpoint inside the container, which is why this is not an
   rsync.
4. Backs up the running binary to `/tmp/ovk-binary-previous` in the container.
5. Unpacks to `/opt/ovk-prototype`, records the deployed SHA in
   `.deployed-commit`, runs `cargo build --release`, restarts the unit.
6. Verifies: health URL returns 200, and `.deployed-commit` matches what was
   asked for. A failed deploy exits non-zero and prints the last 30 journal
   lines.

The whole repo is synced, not just `server/`, because `index.html` is baked
into the binary with `include_str!`.

## One-time setup

Only needed once per repository, by someone with admin rights on it.

**1. Make a deploy key.** A dedicated key, not a personal one, so it can be
revoked on its own:

```sh
ssh-keygen -t ed25519 -f ~/.ssh/ovk_deploy -C 'ovk-demo deploy' -N ''
ssh-copy-id -i ~/.ssh/ovk_deploy.pub root@46.4.94.200
```

**2. Store the private key** as a repository secret named `DEPLOY_SSH_KEY`
(Settings → Secrets and variables → Actions → New repository secret). Paste the
contents of `~/.ssh/ovk_deploy`, including the BEGIN and END lines. Then delete
your local copy of the private key if you do not need it.

**3. Optionally** set `DEPLOY_HOST`, `DEPLOY_USER`, `DEPLOY_CONTAINER` or
`DEPLOY_HEALTH_URL` as repository *variables* to point at a different target.
The defaults are the current demo box.

### Notes

- **Never commit a private key.** A key in the repository is a key handed to
  everyone who has ever cloned it, including after it is deleted, since it stays
  in the history. The secret is the only piece deliberately kept outside these
  files.
- **Forks cannot deploy.** GitHub does not expose secrets to workflows triggered
  from a fork's pull request, by design. Deploys run from the repository that
  holds the secret.
- **Use a dedicated key, not a personal one.** This repository is public, so the
  host address, container name and paths in `deploy.sh` are readable by anyone.
  None of that is a credential and the host is already on a public IP, but it
  does mean the deploy key is the only thing between a reader and the container.
  A key that exists for this one job can be revoked without disturbing anything
  else.
- To restrict who can deploy, the workflow uses the `demo` environment —
  add required reviewers to it under Settings → Environments.

## Rolling back

Deploy an older commit:

```sh
DEPLOY_REF=<sha> ./deploy/deploy.sh
```

or run the workflow by hand with that SHA as the `ref` input. To put back the
previous binary without rebuilding:

```sh
ssh root@46.4.94.200 "lxc exec ovk-demo -- bash -lc \
  'cp /tmp/ovk-binary-previous /opt/ovk-prototype/server/target/release/ovk-prototype-server \
   && systemctl restart ovk-demo'"
```

## Runtime dependencies (separate problem)

A successful deploy does not by itself give you working panel runs. The server
reaches out to two addresses on the author's tailnet:

- the flow hub, `OVK_FLOW_HUB=100.95.66.84:9440`, queue `panel`
- model replies, via OVK's `panel/llm.env`, at `100.70.158.4:11434`

If either is unreachable the static `/mockup` sales demo still serves normally —
it is compiled into the binary — but live runs degrade to labeled mock. Making
*those* portable is a separate change from making the deploy portable.
