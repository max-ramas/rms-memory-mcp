// Package billing owns payment operations.
package billing

import (
	"context"
	"example.com/payments/client"
)

// Service dispatches payments.
type Service struct {
	client *client.Client
}

// Gateway describes the dependency boundary.
type Gateway interface {
	Charge(context.Context, int64) error
}

const defaultCurrency = "USD"

// Charge sends a charge request.
func (s *Service) Charge(ctx context.Context, amount int64) error {
	return s.client.Charge(ctx, amount)
}
