-- Ensure TAP Horizon receipts are de-duplicated by signature
ALTER TABLE tap_horizon_receipts
ADD CONSTRAINT tap_horizon_receipts_signature_unique UNIQUE (signature);
