# Servo CLI

A vibe coded CLI to fetch webpages, mostly for LLM's to use.

## Usage

    servo-cli --url <url> (--html | --md | --accessibility-tree)

- `--url <url>`: page to load; a bare host gets an `https://` prefix.
- `--html`: print the rendered HTML.
- `--md`: print the page as Markdown.
- `--accessibility-tree`: print the accessibility tree.

Pass one output flag. `-h`/`--help` prints usage; a bad invocation exits 2. The tool reads `http_proxy`/`https_proxy` for the proxy, and exits when the page times out after 60 seconds.
