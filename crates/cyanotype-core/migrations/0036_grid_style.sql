-- The per-user photo grid layout: square crops (the default) or justified rows that keep each photo's aspect ratio.
ALTER TABLE user_prefs ADD COLUMN grid_style text NOT NULL DEFAULT 'square' CHECK (grid_style IN ('square', 'justified'));
