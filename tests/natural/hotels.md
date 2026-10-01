# Hotel bookings, 50 turns

File: `Hotel_Bookings_Demand.csv` (Evals `dataframe/`). 119,390 real
bookings at two Portuguese hotels, a City Hotel and a Resort Hotel, arriving
July 2015 to August 2017 (Antonio, Almeida & Nunes, 2019). One conversation,
one prompt per message, in order.

What a careful analyst finds: `agent` and `company` hold the text "NULL"
(16,340 and 112,593 rows); `children` has 4 "NA"; `country` has 488 "NULL";
`adr` (average daily rate) has one negative value (-6.38), a 5,400 outlier
and 1,959 zero-rate stays; 180 bookings have no adults, children or babies;
and the arrival date is split over three columns, the month spelled out.

## Excel

1. Open Hotel_Bookings_Demand.csv in Excel.
2. What is this? A short description of the data and the period it covers.
3. How many bookings are there for each of the two hotels?
4. Save it as a workbook called Hotel Bookings.xlsx and work in that from now on.
5. Rename the sheet to Bookings and freeze the header row.
6. The agent and company columns are full of the word NULL. Make those cells empty instead.
7. A few rows have NA for children. Treat those as 0.
8. Add an Arrival Date column that puts the year, month and day together as a real date.
9. Add a Nights column: weekend nights plus week nights.
10. Add a Revenue column: the average daily rate times the nights, only for bookings that weren't cancelled.
11. Is anything odd about the average daily rate? Look and tell me; don't change anything.
12. Highlight any rate below 0 or above 1,000 in red, so we can see them.
13. Turn the bookings into an Excel table with a light style.
14. What's the overall cancellation rate, and is it different between the two hotels?
15. Make a sheet called Overview with bookings, cancellations and the cancellation rate for each hotel.
16. Add the average lead time and the average number of nights to that table.
17. Under it, add the same figures by market segment.
18. Sort the market segment table by cancellation rate, highest first.
19. Hmm, put the market segments back in alphabetical order.
20. Make a pivot table of revenue by arrival month and hotel, with the months in calendar order, not alphabetical.
21. Add a slicer for the year.
22. And one for customer type.
23. Make a line chart from the pivot showing revenue by month for each hotel.
24. Which ten countries send the most guests? Put them in a small table on the Overview sheet.
25. The country codes aren't very readable. Put the country's name next to each code in that table.
26. Make a bar chart of those ten countries.
27. Move that chart to the right of the country table so they sit side by side.
28. Does a deposit change how often people cancel? Show the cancellation rate by deposit type.
29. That looks backwards: non-refundable bookings cancel more? Add a sentence under the table saying what you think is going on.
30. Build a Dashboard sheet: the revenue chart, the countries chart, the cancellation rate for each hotel as big numbers, and the slicers. Tidy, nothing overlapping.
31. Add a button on the dashboard that refreshes the pivot and the charts.
32. Delete the Revenue column from the Bookings sheet.
33. No wait, the pivot uses it. Put it back.
34. Show all the money in euros.
35. Save the workbook.

## Word

36. Create a new Word document called Hotel Demand Report.
37. Title it "Hotel Demand, 2015 to 2017", with a short subtitle saying what data it's based on.
38. Write a short executive summary: the three things that matter most, with numbers.
39. Add a section on cancellations, with the hotel and market segment tables from the workbook.
40. Add a section on seasonality with the monthly revenue chart.
41. Add a section on where guests come from, with the countries table and chart.
42. Add a Data Quality section: each problem you found in the data and what you did about it.
43. The executive summary reads like a list. Rewrite it as one flowing paragraph.
44. Add a table of contents and page numbers, and save it.

## PowerPoint

45. Create a new PowerPoint deck called Hotel Demand Briefing.
46. A title slide, then one slide each for cancellations, seasonality and guest origins, using the charts.
47. Add a slide with three recommendations for the revenue manager.
48. Add speaker notes to every slide.
49. Move the recommendations slide so it comes straight after the title slide.
50. Save it, then tell me which number in the deck you're least sure of, and why.
