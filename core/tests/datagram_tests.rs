extern crate hamesh;

use hamesh::*;
#[cfg(test)]
mod tests {
    use hamesh::control_datagram::ControlDatagram;
    use std::ops::Index;
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn fragmentation_results_in_correct_size() {
        for i in 0..1500 {
            let data = vec![2; 10_000 + i];
            let encoded_data = base64::encode(&data);
            let client_data_datagram = control_datagram::ControlDatagram::client_data(
                "0001",
                0,
                settings_models::ClientDataSettings {
                    protocol: settings_models::Protocol::Tcp,
                    host_port: 80,
                    host_client_port: 49152,
                },
                &encoded_data,
            );
            let max_fragment_size = 1400;
            let fragments = control_datagram::ControlDatagram::fragments(
                client_data_datagram,
                max_fragment_size as u16,
            )
            .unwrap();
            assert!(fragments.len() >= 1);
            for fragment in &fragments[..fragments.len() - 1] {
                assert!(
                    fragment.to_vec().unwrap().len() == max_fragment_size
                        || fragment.to_vec().unwrap().len() == max_fragment_size - 1
                );
            }
            assert!(&fragments.last().unwrap().to_vec().unwrap().len() <= &max_fragment_size);
        }
    }

    #[test]
    fn fragmentation_data_integrity() {
        for i in 0..1500 {
            let data = vec![2; 10_000 + i];
            let encoded_data = base64::encode(&data);
            let client_data_datagram = control_datagram::ControlDatagram::client_data(
                "0001",
                0,
                settings_models::ClientDataSettings {
                    protocol: settings_models::Protocol::Tcp,
                    host_port: 80,
                    host_client_port: 49152,
                },
                &encoded_data,
            );
            let max_fragment_size = 1400;
            let fragments = control_datagram::ControlDatagram::fragments(
                client_data_datagram,
                max_fragment_size as u16,
            )
            .unwrap();
            assert!(fragments.len() >= 1);
            let mut data = "".to_string();
            for fragment in &fragments[..fragments.len() - 1] {
                assert!(
                    fragment.to_vec().unwrap().len() == max_fragment_size
                        || fragment.to_vec().unwrap().len() == max_fragment_size - 1
                );
                data.push_str(&fragment.content["data"])
            }
            assert!(&fragments.last().unwrap().to_vec().unwrap().len() <= &max_fragment_size);
            data.push_str(&fragments.last().unwrap().content["data"]);

            serde_json::from_str::<ControlDatagram>(&data)
                .map_err(|e| panic!("error for: {data}\n{e}"))
                .unwrap();
        }
    }
}
