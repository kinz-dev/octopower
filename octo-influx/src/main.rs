// Copyright 2022 the octopower authors.
// This project is dual-licensed under Apache 2.0 and MIT terms.
// See LICENSE-APACHE and LICENSE-MIT for details.

use chrono::{DateTime, Utc};
use eyre::Report;
use influxdb::{Client, WriteQuery};
use influxdb::InfluxDbWriteable;
use log::info;
use regex::Regex;

use config::{Config, get_influxdb_client};
use octopower::{
    authenticate, AuthToken, get_account, get_consumption, MeterType, get_standard_unit_rates,
    results::consumption::Consumption,
    results::standing_unit_rate::StandingUnitRate
};

mod config;

#[tokio::main]
async fn main() -> Result<(), Report> {
    pretty_env_logger::init();

    let config = Config::from_file()?;
    let influxdb_client = get_influxdb_client(&config.influxdb)?;
    println!("************************");
    println!(" influxdb_client = {:?}", influxdb_client);
    let token = authenticate(&config.octopus.email_address, &config.octopus.password).await?;
    let account = get_account(&token, &config.octopus.account_id).await?;

    let mut tariff_code: &str = "";
    let mut product_code: &str = "";
    let re = Regex::new(r#"[A-Z]+-[A-Z]+-\d{2}-\d{2}-\d{2}"#).unwrap();

    for property in &account.properties {
        info!("Property {}", property.address_line_1);
        for electricity_meter_point in &property.electricity_meter_points {
            info!("Electricity MPAN {}", electricity_meter_point.mpan);
            if let Some(last_agreement) = electricity_meter_point.agreements.last() {
                info!("Latest agreement {:?}", last_agreement);
                tariff_code = &last_agreement.tariff_code
            }
            for meter in &electricity_meter_point.meters {
                info!("Meter serial {}", meter.serial_number);
                import_consumption_readings(&token, MeterType::Electricity, &electricity_meter_point.mpan,
                    &meter.serial_number, &influxdb_client, "consumption", config.num_readings) .await?;
            }
        }
        for gas_meter_point in &property.gas_meter_points {
            info!("Gas MPRN {}", gas_meter_point.mprn);
            for meter in &gas_meter_point.meters {
                info!("Meter serial {}", meter.serial_number);
                import_consumption_readings(&token, MeterType::Gas, &gas_meter_point.mprn, &meter.serial_number,
                    &influxdb_client, "consumption", config.num_readings).await?;
            }
        }

        // *************************
        // Unit rate
        // *************************
        info!("Tariff code = {}", tariff_code.to_string());
        // Match the regular expression against the input
        if let Some(captured) = re.find(tariff_code) {
            // Extract the matched substring
            product_code = &tariff_code[captured.start()..captured.end()];
        }
        info!("Extracted product code : {}", product_code);
        import_unit_rates(&token, product_code, tariff_code, &influxdb_client, "rates", config.unit_rates_num_readings).await?;
    }

    Ok(())
}

async fn import_consumption_readings(token: &AuthToken, meter_type: MeterType, mpxn: &str,
    serial: &str, influxdb_client: &Client, measurement: &str, num_readings: usize) -> Result<(), Report> {
    let consumption =
        get_consumption(token, meter_type, mpxn, serial, 0, num_readings, None).await?;
    info!("{:?} consumption: {}/{} records", meter_type, consumption.results.len(), consumption.count);
    let points = consumption
        .results
        .into_iter()
        .map(|reading| get_consumption_write_query(measurement, meter_type, mpxn, serial, reading))
        .collect::<Vec<WriteQuery>>();

    let result = influxdb_client.query(points).await;
    info!("Writing consumption data to influxdb == {:?}", result);
    assert!(result.is_ok(), "Write result was not okay");
    Ok(())
}

async fn import_unit_rates(token: &AuthToken, product_code: &str, tariff_code: &str,
    influxdb_client: &Client, measurement: &str, num_readings: usize) -> Result<(), Report> {
    let rates = get_standard_unit_rates(token, MeterType::Electricity, product_code, tariff_code, 0, num_readings).await?;
    info!("{:?} rates: {}/{} records", MeterType::Electricity, rates.results.len(), rates.count);
    let points = rates
        .results
        .into_iter()
        .map(|rate| get_unit_rates_write_query(measurement, product_code, tariff_code, rate))
        .collect::<Vec<WriteQuery>>();

    let result = influxdb_client.query(points).await;
    info!("Writing unit rate data to influxdb == {:?}", result);
    assert!(result.is_ok(), "Write result was not okay");
    Ok(())
}

#[derive(InfluxDbWriteable)]
struct ConsumptionReading {
    time: DateTime<Utc>,
    #[influxdb(tag)] meter_type: String,
    #[influxdb(tag)] mpxn: String,
    #[influxdb(tag)] serial: String,
    consumption: f64
}
fn get_consumption_write_query(
    measurement: &str,
    meter_type: MeterType,
    mpxn: &str,
    serial: &str,
    consumption: Consumption
) -> WriteQuery {
    ConsumptionReading {
        time: consumption.interval_start,
        meter_type: meter_type.to_string(),
        mpxn: mpxn.to_string(),
        serial: serial.to_string(),
        consumption: consumption.consumption as f64
    }.into_query(measurement)
}

#[derive(InfluxDbWriteable)]
struct UnitRatesReading {
    time: DateTime<Utc>,
    #[influxdb(tag)] product_code: String,
    #[influxdb(tag)] tariff_code: String,
    rate: f64
}

fn get_unit_rates_write_query(
    measurement: &str,
    product_code: &str,
    tariff_code: &str,
    unit_rate: StandingUnitRate
) -> WriteQuery {
    UnitRatesReading {
        time: unit_rate.valid_from,
        product_code: product_code.to_string(),
        tariff_code: tariff_code.to_string(),
        rate: unit_rate.value_inc_vat as f64
    }.into_query(measurement)
}
