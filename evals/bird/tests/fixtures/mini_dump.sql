--
-- PostgreSQL database dump
--

SET statement_timeout = 0;
SET client_encoding = 'UTF8';
SELECT pg_catalog.set_config('search_path', '', false);
SET default_table_access_method = heap;

--
-- Name: account; Type: TABLE; Schema: public; Owner: xiaolongli
--

CREATE TABLE public.account (
    account_id bigint DEFAULT '0'::bigint NOT NULL,
    frequency text
);


ALTER TABLE public.account OWNER TO xiaolongli;

CREATE TABLE public.customers (
    customerid bigint NOT NULL,
    segment text
);


ALTER TABLE public.customers OWNER TO xiaolongli;

CREATE TABLE public.atom (
    atom_id text NOT NULL
);


ALTER TABLE public.atom OWNER TO xiaolongli;

CREATE SEQUENCE public.customers_customerid_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER TABLE public.customers_customerid_seq OWNER TO xiaolongli;

ALTER SEQUENCE public.customers_customerid_seq OWNED BY public.customers.customerid;

ALTER TABLE ONLY public.customers ALTER COLUMN customerid SET DEFAULT nextval('public.customers_customerid_seq'::regclass);

COPY public.account (account_id, frequency) FROM stdin;
1	monthly
2	CREATE TABLE public.customers (id int);
\.


COPY public.customers (customerid, segment) FROM stdin;
10	SME
\.


COPY public.atom (atom_id) FROM stdin;
a1
\.


SELECT pg_catalog.setval('public.customers_customerid_seq', 10, true);


ALTER TABLE ONLY public.account
    ADD CONSTRAINT account_pkey PRIMARY KEY (account_id);


ALTER TABLE ONLY public.customers
    ADD CONSTRAINT customers_pkey PRIMARY KEY (customerid);


ALTER TABLE ONLY public.atom
    ADD CONSTRAINT atom_pkey PRIMARY KEY (atom_id);


CREATE INDEX idx_customers_segment
    ON public.customers USING btree (segment);

--
-- PostgreSQL database dump complete
--
