-- The site has a place: public dates are rendered in this IANA zone instead
-- of UTC. An author name feeds feeds and structured data (the site title
-- stands in while it is empty), and a one-slot backup makes "restore the
-- default theme" reversible. Defaults keep every existing site unchanged.
ALTER TABLE site_settings ADD COLUMN timezone TEXT NOT NULL DEFAULT 'UTC';
ALTER TABLE site_settings ADD COLUMN author_name TEXT NOT NULL DEFAULT '';
ALTER TABLE site_settings ADD COLUMN custom_css_backup TEXT;
