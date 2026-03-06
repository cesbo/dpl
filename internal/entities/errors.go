// Package entities defines shared types and errors for all entity kinds.
package entities

import "errors"

// ErrConflict indicates a deploy is already in progress for the entity.
var ErrConflict = errors.New("deploy already in progress")

// ErrUnsupportedType indicates the config.yaml has an unrecognized type field.
var ErrUnsupportedType = errors.New("unsupported type")
