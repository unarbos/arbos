// Package builtin registers sample extensions that ship with arbos.
package builtin

import (
	"context"
	"fmt"
	"reflect"
	"runtime"

	"github.com/unarbos/arbos/internal/extension"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/jsonschema"
)

// All returns the built-in extensions loaded by piwire.
func All() []extension.Extension {
	return []extension.Extension{VersionExt}
}

type versionArgs struct {
	Detail bool `json:"detail,omitempty" desc:"Include Go runtime version."`
}

// VersionExt registers a read-only arbos_version tool.
func VersionExt(api extension.API) error {
	schema, err := jsonschema.Reflect(reflect.TypeOf(versionArgs{}))
	if err != nil {
		return err
	}
	spec := tool.NewSpec("arbos_version", "Return the arbos host version string.", true,
		func(_ context.Context, a versionArgs) (string, error) {
			if a.Detail {
				return fmt.Sprintf("arbos pi (go %s)", runtime.Version()), nil
			}
			return "arbos pi", nil
		})
	return api.RegisterTool(spec, schema)
}
