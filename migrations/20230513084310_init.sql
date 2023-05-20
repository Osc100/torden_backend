CREATE TABLE company (
    id serial PRIMARY KEY,
    name varchar(255) NOT NULL UNIQUE
);

CREATE TYPE account_role AS ENUM ('agent', 'manager', 'superuser');

CREATE TABLE account (
    id serial PRIMARY KEY,
    email varchar(255) NOT NULL UNIQUE,
    password varchar(255) NOT NULL, -- hashed
    --
    first_name varchar(255) NOT NULL,
    last_name varchar(255) NOT NULL,
    role account_role NOT NULL,
    company_id integer NOT NULL REFERENCES company(id),
    created timestamp DEFAULT now() NOT NULL
);

CREATE TABLE chat (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    created timestamp DEFAULT now() NOT NULL,
    company_id integer NOT NULL REFERENCES company(id)
);

CREATE TYPE message_role AS ENUM ('user', 'system', 'assistant');

CREATE TABLE message (
    id serial PRIMARY KEY,
    chat_id uuid NOT NULL REFERENCES chat(id),
    role message_role NOT NULL,
    text text NOT NULL,
    created timestamp DEFAULT now() NOT NULL,
    account_id integer NULL REFERENCES account(id)
);