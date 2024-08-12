mod version;

use gtk::glib::clone;
use gtk::prelude::*;
use gtk::{glib, Align, Application, ApplicationWindow, Box, Button, ButtonsType, ComboBox, ComboBoxText, Dialog, DialogFlags, Entry, Label, MessageDialog, MessageType, Orientation, ResponseType, Separator};
use hamesh::ip_version::IpVersion;
use hamesh::settings_models::Protocol;
use log::{debug, error, info, trace, LevelFilter};
use simple_logger::SimpleLogger;
use std::num::ParseIntError;
use hamesh::version::VERSION as VERSION;
use crate::version::VERSION as DESKTOP_VERSION;

// #[tokio::main]
fn main() {
    SimpleLogger::new()
        .with_level(LevelFilter::Debug)
        .init()
        .unwrap();
    // Create a new application with the builder pattern
    let app = Application::builder()
        .application_id("com.blackmidori.hamesh-desktop")
        .build();
    app.connect_activate(on_activate);
    // Run the application
    app.run();
}

fn on_activate(application: &Application) {
    // … create a new window …
    let window = ApplicationWindow::new(application);
    window.set_title(Some("Hamesh Desktop"));
    window.set_default_size(300, 500);
    let vbox = Box::new(Orientation::Vertical, 5);
    window.set_child(Some(&vbox));
    window.present();
    // let button = Button::with_label("Hamesh Desktop");
    // button.connect_clicked(clone!(@weak window => move |_| window.close()));
    // window.set_child(Some(&button));

    let connections_box = Box::new(Orientation::Vertical, 5);
    let button = Button::with_label("Add Forwarding");
    button.connect_clicked(clone!(@weak window, @weak connections_box => move|_|
        on_add_forwarding(window, move |ip_version, protocol, local_port, remote_port| {
            let subscription_id = Label::new(Some(format!("⏳ {} {}:{}/{}",ip_version, local_port,remote_port,protocol).as_str()));
            connections_box.append(&subscription_id);
        })
    ));
    vbox.append(&button);

    vbox.append(&connections_box);

    let spacer = Box::new(Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    vbox.append(&spacer);

    let bottom_vbox = Box::new(Orientation::Vertical, 0);
    vbox.append(&bottom_vbox);

    let separator = Separator::new(Orientation::Vertical);
    bottom_vbox.append(&separator);

    let bottom_hbox = Box::new(Orientation::Horizontal, 5);
    bottom_vbox.append(&bottom_hbox);

    let hyperlink_label = Label::new(Some("<a href=\"https://hamesh.blackmidori.com\">hamesh.blackmidori.com</a>"));
    hyperlink_label.set_use_markup(true);
    bottom_hbox.append(&hyperlink_label);

    let bottom_spacer = Box::new(Orientation::Horizontal, 0);
    bottom_spacer.set_hexpand(true);
    bottom_hbox.append(&bottom_spacer);

    let version_label = Label::new(Some(format!("v{DESKTOP_VERSION} (core v{VERSION})").as_str()));
    bottom_hbox.append(&version_label);

    window.set_child(Some(&vbox));
}

fn on_add_forwarding<F: Fn(IpVersion, Protocol, u16, u16) + 'static>(
    window: ApplicationWindow,
    on_add: F,
) {
    // Create and configure the dialog
    let dialog = Dialog::with_buttons(
        Some("Add Forwarding"),
        Some(&window),
        DialogFlags::MODAL,
        &[("Add", ResponseType::Ok)],
    );
    dialog.set_title(Some("Add Forwarding"));

    let ip_version_combo_box = ComboBoxText::new();
    ip_version_combo_box.append_text("IPv4");
    ip_version_combo_box.append_text("IPv6");
    ip_version_combo_box.set_active(Some(0));

    let protocol_combo_box = ComboBoxText::new();
    protocol_combo_box.append_text("TCP");
    protocol_combo_box.append_text("UDP");
    protocol_combo_box.set_active(Some(0));

    let local_port = Entry::new();
    local_port.set_placeholder_text(Some("Local Port"));
    let local_port_error = Label::new(None);
    local_port_error.hide();

    let remote_port = Entry::new();
    remote_port.set_placeholder_text(Some("Remote Port"));
    let remote_port_error = Label::new(None);
    remote_port_error.hide();

    let content_area = dialog.content_area();
    content_area.append(&ip_version_combo_box);
    content_area.append(&protocol_combo_box);
    content_area.append(&local_port);
    content_area.append(&local_port_error);
    content_area.append(&remote_port);
    content_area.append(&remote_port_error);

    // Run the dialog and handle user response
    // Connect response signal to the dialog
    let dialog_clone = dialog.clone();
    dialog.connect_response(move |dialog, response| match response {
        ResponseType::Ok => {
            let (local_port_u16, remote_port_u16) = match (
                local_port.text().parse::<u16>(),
                remote_port.text().parse::<u16>(),
            ) {
                (Ok(local_port_i16), Ok(remote_port_i16)) => {
                    local_port_error.hide();
                    remote_port_error.hide();
                    (local_port_i16, remote_port_i16)
                }
                (local_port_result, remote_port_result) => {
                    local_port_error.hide();
                    remote_port_error.hide();
                    _ = local_port_result.inspect_err(|error| {
                        local_port_error.set_text(&format!("{}", error));
                        local_port_error.show();
                    });
                    _ = remote_port_result.inspect_err(|error| {
                        remote_port_error.set_text(&format!("{}", error));
                        remote_port_error.show();
                    });
                    return;
                }
            };
            let ip_version = match ip_version_combo_box.active_text() {
                Some(text) => match text.as_str() {
                    "IPv4" => IpVersion::Ipv4,
                    "IPv6" => IpVersion::Ipv6,
                    fallback => return error!("Expected: IPv4 or IPv6. Actual: {fallback}"),
                },
                _ => return error!("No ip version selected"),
            };

            let protocol = match protocol_combo_box.active_text() {
                Some(text) => match text.as_str() {
                    "TCP" => Protocol::Tcp,
                    "UDP" => Protocol::Udp,
                    fallback => return error!("Expected: TCP or UDP. Actual: {fallback}"),
                },
                _ => return error!("No protocol selected"),
            };
            on_add(ip_version, protocol, local_port_u16, remote_port_u16);
            dialog.destroy();
        }
        ResponseType::DeleteEvent => {}
        fallback => {
            debug!("Cannot handle {fallback}");
        }
    });
    dialog.present();
}
