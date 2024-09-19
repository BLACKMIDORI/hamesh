use alloc::boxed;
use alloc::rc::Rc;
use gtk::glib::clone;
use gtk::prelude::{
    BoxExt, ButtonExt, ComboBoxExtManual, DialogExt, EditableExt, EntryExt, GtkWindowExt, WidgetExt,
};
use gtk::{
    glib, ApplicationWindow, Box, Button, ComboBoxText, Dialog, DialogFlags, Entry, Label,
    Orientation, ResponseType, Widget,
};
use hamesh::ip_version::IpVersion;
use hamesh::settings_models::Protocol;
use hamesh::{
    connect_peers, get_peers, get_socket, get_subscription, read_peer_subscription_from_stdin,
};
use log::{error, info, warn};
use std::error::Error;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::Receiver;

pub struct ForwardingItem {
    pub widget: Widget,
    ip_version: IpVersion,
    protocol: Protocol,
    local_port: u16,
    remote_port: u16,
    pub change_status: boxed::Box<dyn Fn(&str)>,
    change_subscription: boxed::Box<dyn Fn(&str)>,
    hide_subscription: boxed::Box<dyn Fn()>,
    hide_button: boxed::Box<dyn Fn()>,
    subscription_input_receiver: Receiver<String>,
}

impl ForwardingItem {
    pub fn new(
        window: ApplicationWindow,
        ip_version: IpVersion,
        protocol: Protocol,
        local_port: u16,
        remote_port: u16,
    ) -> ForwardingItem {
        let r#box = Box::new(Orientation::Horizontal, 10);
        let status_text = Label::new(Some("🆕"));
        let subscription_read_only = Entry::new();
        subscription_read_only.set_editable(false);
        subscription_read_only.set_text("My Subscription Id");
        let subscription_read_only_clone = subscription_read_only.clone();
        let forwarding_settings = Label::new(Some(
            format!("{} {}:{}/{}", ip_version, local_port, remote_port, protocol).as_str(),
        ));
        let button = Button::with_label("🔗");
        let (subscription_input_sender, subscription_input_receiver) =
            tokio::sync::mpsc::channel(1024);
        button.connect_clicked(clone!(@weak window => move|_|{
                let subscription_input_sender = subscription_input_sender.clone();
            on_connect_pressed(window, move |subscription_id| {
                let subscription_input_sender = subscription_input_sender.clone();
                tokio::spawn(async move {subscription_input_sender.send(subscription_id).await});
            })
        }
        ));
        r#box.append(&status_text);
        r#box.append(&forwarding_settings);
        r#box.append(&subscription_read_only);
        r#box.append(&button);

        ForwardingItem {
            widget: r#box.into(),
            ip_version,
            protocol,
            local_port,
            remote_port,
            change_status: boxed::Box::new(move |new_status| {
                status_text.set_text(new_status);
            }),
            change_subscription: boxed::Box::new(move |new_status| {
                subscription_read_only.show();
                subscription_read_only.set_text(new_status);
            }),
            hide_subscription: boxed::Box::new(move || {
                subscription_read_only_clone.hide();
            }),
            hide_button: boxed::Box::new(move || {
                button.hide();
            }),
            subscription_input_receiver: subscription_input_receiver,
        }
    }

    pub async fn start(self) {
        (self.change_status)("⏳");
        let ip_version = self.ip_version.clone();
        tokio::join!(async {
            let socket = get_socket(ip_version).await;
            let peers_result = {
                let (peer_subscription_id_sender, peer_subscription_id_receiver) =
                    tokio::sync::mpsc::channel(1024);
                let peer_subscription_id_sender_clone = peer_subscription_id_sender.clone();
                let handler = tokio::spawn(async move {
                    let mut subscription_input_receiver = self.subscription_input_receiver;
                    match subscription_input_receiver.recv().await {
                        Some(peer_subscription_id) => {
                            _ = peer_subscription_id_sender_clone
                                .send(Some(peer_subscription_id))
                                .await;
                        }
                        None => {}
                    }
                });
                info!(
                    "requesting subscription for {} {}:{}/{} ...",
                    self.ip_version, self.local_port, self.remote_port, self.protocol
                );
                let (client_endpoint, subscription_id) = get_subscription(&socket)
                    .await
                    .map_err(|e| format!("{e}"))?;
                (&self.change_subscription)(&subscription_id);
                let join_handle = tokio::spawn(get_peers(
                    subscription_id,
                    client_endpoint,
                    peer_subscription_id_receiver,
                ));
                let result = join_handle.await.map_err(|e|format!("{e}"))?;
                (self.hide_button)();
                _ = peer_subscription_id_sender.send(None).await;
                handler.abort();
                result
            };
            (&self.hide_subscription)();
            match peers_result {
                Err(error) => {
                    error!("{}", error);
                    (&self.change_status)("❌");
                    (&self.change_subscription)(&error);
                }
                Ok(peers) => {
                    (&self.change_status)("🔗");
                    (&self.change_subscription)(&format!(
                        "peer = {}:{}",
                        &peers.1.address, &peers.1.port
                    ));
                    let (status_sender,mut status_receiver) = tokio::sync::mpsc::channel(1024);
                    tokio::spawn(async move {
                        match connect_peers(socket, &peers.0, &peers.1)
                        .await
                        .map_err(|e| format!("{e}"))
                        {
                            Err(error) =>{
                                error!("{}", error);
                                if let Err(e) = status_sender.send("⛓️‍💥").await{
                                    error!("status_sender.send(): {e}")
                                }
                            }
                            Ok(_) => {
                                if let Err(e) = status_sender.send("🛑").await{
                                    error!("status_sender.send(): {e}")
                                }
                            }
                        };
                    });
                    loop{
                        match status_receiver.recv().await{
                            Some(new_status) => {
                                (self.change_status)(new_status);
                            }
                            None => {
                                break;
                            }
                        }
                    }
                }
            };
            Ok::<(), String>(())
        });
    }
}

fn on_connect_pressed<F: Fn(String) + 'static>(
    window: ApplicationWindow,
    on_subscription_input_confirm: F,
) {
    // Create and configure the dialog
    let dialog = Dialog::with_buttons(
        Some("Connect to subscription"),
        Some(&window),
        DialogFlags::MODAL,
        &[("Connect", ResponseType::Ok)],
    );
    dialog.set_title(Some("Connect to subscription"));

    let subscription_id_entry = Entry::new();
    subscription_id_entry.set_placeholder_text(Some("Peer Subscription Id"));

    let content_area = dialog.content_area();
    content_area.append(&subscription_id_entry);

    // Run the dialog and handle user response
    // Connect response signal to the dialog
    dialog.connect_response(move |dialog, response| match response {
        ResponseType::Ok => {
            on_subscription_input_confirm(subscription_id_entry.text().as_str().to_string());
            dialog.destroy();
        }
        ResponseType::DeleteEvent => {}
        fallback => {
            warn!("Cannot handle {fallback}");
        }
    });
    dialog.present();
}
