/*
  oxy:
    database: local
    embed:
      - How many stores do I have
*/
SELECT COUNT(DISTINCT Store) AS number_of_stores
FROM oxymart;