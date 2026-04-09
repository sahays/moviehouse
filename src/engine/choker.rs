use std::net::SocketAddr;

use rand::seq::SliceRandom;

use crate::peer::manager::PeerManager;
use crate::peer::connection::PeerCommand;

/// Number of regular unchoke slots.
const UNCHOKE_SLOTS: usize = 4;

/// Run the choking algorithm.
///
/// Every 10 seconds: unchoke the top UNCHOKE_SLOTS peers by download rate.
/// Every 30 seconds (optimistic_round): additionally unchoke 1 random choked+interested peer.
pub fn run_choking_algorithm(
    peer_manager: &PeerManager,
    optimistic_round: bool,
) -> Vec<(SocketAddr, PeerCommand)> {
    let mut commands = Vec::new();
    let peers = peer_manager.connected_peers();

    if peers.is_empty() {
        return commands;
    }

    // Rank peers by download rate (bytes_downloaded as proxy)
    let mut ranked: Vec<(SocketAddr, u64)> = peers
        .iter()
        .filter_map(|addr| {
            peer_manager
                .peer_state(addr)
                .map(|s| (*addr, s.bytes_downloaded))
        })
        .collect();

    ranked.sort_by(|a, b| b.1.cmp(&a.1)); // highest first

    // Top UNCHOKE_SLOTS get unchoked
    let mut unchoked: Vec<SocketAddr> = Vec::new();
    for (addr, _) in ranked.iter().take(UNCHOKE_SLOTS) {
        unchoked.push(*addr);
        commands.push((*addr, PeerCommand::SendUnchoke));
    }

    // Optimistic unchoke: pick one random choked peer
    if optimistic_round {
        let choked: Vec<SocketAddr> = ranked
            .iter()
            .skip(UNCHOKE_SLOTS)
            .map(|(addr, _)| *addr)
            .collect();

        if let Some(addr) = choked.choose(&mut rand::thread_rng()) {
            unchoked.push(*addr);
            commands.push((*addr, PeerCommand::SendUnchoke));
        }
    }

    // Choke everyone else
    for (addr, _) in &ranked {
        if !unchoked.contains(addr) {
            commands.push((*addr, PeerCommand::SendChoke));
        }
    }

    commands
}
