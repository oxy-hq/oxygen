# Evidence coverage (debit_card_specializing)

One line per evidence string (verbatim) → the member(s) that carry it.

- A customer's consumption for a month is the amount of fuel recorded against them for that month; by 'consumed any fuel in 2013' I mean they had fuel recorded against them in at least one month of 2013. → `yearmonth.consumption`, `yearmonth.year` / `yearmonth.year_month`, `yearmonth.consuming_customer_count`, `yearmonth.customer_month_count`, `yearmonth` view description (row presence = consumed that month; zero/negative rows noted, no filter)
- By "gas" I mean gasoline (motor fuel) purchases, not natural gas. → `products` view description (glossary: Natural/Super/Special = gasoline, CNG/NPG = natural gas), `products.product_description`, `yearmonth` view description
- Consumption means the amount spent through a customer's fuel cards. → `yearmonth.consumption`, `yearmonth.total_consumption`, `yearmonth` view description, `transactions_1k` view description (consumption is not derived from transactions)
- Consumption means the monthly consumption amount recorded per customer. → `yearmonth.consumption`, `yearmonth.total_consumption`, `yearmonth.average_consumption`, `yearmonth.year_month`
- Consumption means the total fuel a customer purchased in a month on their fuel card; a customer counts for a month if they had any consumption that month. → `yearmonth.consumption`, `yearmonth.total_consumption`, `yearmonth.month` / `yearmonth.year_month`, `yearmonth.consuming_customer_count`, `yearmonth` view description
- Czech Republic means the gas station is in the CZE country. → `gasstations.country` (sample 'CZE'), `gasstations.czech_gas_station_count`, `gasstations` / `transactions_1k` view descriptions
- EUR-paying / CZK-paying: a customer whose account currency is EUR / CZK → `customers.currency` (samples 'EUR', 'CZK'), `customers.eur_customer_count`, `customers.czk_customer_count`, `customers` view description
- KAM is one of our client segments. → `customers.customer_segment` (sample 'KAM'), `customers.kam_customer_count`, `customers` view description

## Description-file formula

- `transactions_1k.Price` value description "total price = Amount x Price" → `transactions_1k.total_price` (SUM(amount*price)), `transactions_1k.line_total_price`; data caveat (price looks like a line total) → `transactions_1k.total_price_recorded`, `transactions_1k.unit_price`, view description.

## Codes in the data but not in the description CSVs

- `customers.segment`: 'SME', 'LAM' (only 'KAM' named by evidence) → listed as samples, not expanded.
- `gasstations.country`: 'SVK' (Slovakia; only 'CZE' named by evidence) → sample + description.
- `gasstations.segment`: 'Premium', 'Value for money', 'Discount', 'Noname', 'Other' → samples, not expanded.
- `customers.currency`: 'CZK', 'EUR' → decoded from evidence.
- `yearmonth.date`: 'YYYYMM' text, 201112..201209 and 201301..201311 (201210..201212 absent) → description.
