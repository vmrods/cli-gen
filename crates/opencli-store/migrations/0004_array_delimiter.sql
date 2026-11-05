-- How array parameter values are joined (`a,b`); NULL repeats the parameter.
ALTER TABLE parameters ADD COLUMN array_delimiter TEXT;
