# gopkg.in/yaml.v3 parity corpus

`../../fixtures/yaml_v3_go.json` holds how gopkg.in/yaml.v3 **v3.0.1** (the version
CLIProxyAPI v8.0.35, `ae3ee3be`, pins in `go.mod`) reads:

- the 119 edge cases in `../../fixtures/yaml_v3/*.yaml`, written byte for byte by
  `gen_corpus.py` (`.gitattributes` keeps checkouts from converting their line
  endings);
- every `yaml` string in cpa-core's other `../../fixtures/*_go.json` files, the
  config.yaml-shaped inputs other tests already replay;
- the reference's `config.example.yaml`.

Each case records `yaml.Unmarshal` into `interface{}`, a `yaml.Decoder` over every
document, and `yaml.Unmarshal` into a struct with the field kinds of Go's config.
`main.go` describes the token format. `../../yaml_v3_go.rs` compares today's parser
with the recording and pins the differences in
`../../fixtures/yaml_v3_known_differences.json`; `docs/YAML-PARSER.md` uses it to choose
the replacement parser.

Regenerate with Go 1.27.3, network denied. The recorder only needs yaml.v3 (from the
module cache), so it runs from a throwaway module outside the repository:

```sh
here=$PWD                                    # this directory
reference=/absolute/path/to/CLIProxyAPI      # at ae3ee3be, for config.example.yaml
work=$(mktemp -d)
cp main.go "$work/"
printf 'module yamlv3recorder\n\ngo 1.22\n\nrequire gopkg.in/yaml.v3 v3.0.1\n' > "$work/go.mod"
python3 gen_corpus.py ../../fixtures/yaml_v3        # only after editing the corpus
(cd "$work" && GOFLAGS=-mod=mod GOPROXY=off go run . "$here/../../fixtures" \
  "$reference/config.example.yaml" "$here/../../fixtures/yaml_v3_go.json")
```

Before regenerating after a corpus edit, delete `../../fixtures/yaml_v3/*.yaml` so a
removed case does not linger. Then re-bless the known differences and review the diff:

```sh
YAML_V3_BLESS=1 cargo test -p cpa-core --test yaml_v3_go
```
