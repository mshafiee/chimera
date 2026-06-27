"""
Network Features for Scout

Analyzes wallet and token network relationships.
This module provides:
- Wallet centrality in transaction graph (PageRank-style)
- Follower network analysis
- Token co-holding patterns with successful wallets
- Graph-based clustering for sybil detection

Usage:
    extractor = NetworkFeatures()
    features = extractor.extract_features(wallet_address, transaction_graph)
"""

import logging
from typing import Dict, List, Optional, Any
from collections import Counter
import numpy as np

logger = logging.getLogger(__name__)

# Try to import networkx for graph analysis
try:
    import networkx as nx
    NETWORKX_AVAILABLE = True
except ImportError:
    NETWORKX_AVAILABLE = False
    logger.warning("networkx not available - install with: pip install networkx")


class NetworkFeatures:
    """
    Network-based features for wallet analysis.

    Features:
    - Wallet centrality (PageRank, degree, betweenness)
    - Token co-holding patterns
    - Follower network analysis
    - Sybil wallet clustering
    """

    def __init__(self):
        """Initialize network features."""
        self.graph_cache = {}

    def extract_features(
        self,
        wallet_address: str,
        transaction_graph: Optional[Dict[str, Any]] = None,
        token_holdings: Optional[Dict[str, float]] = None,
        known_successful_wallets: Optional[List[str]] = None
    ) -> Dict[str, Any]:
        """
        Extract network-based features for a wallet.

        Args:
            wallet_address: Wallet address to analyze
            transaction_graph: Optional transaction graph data
            token_holdings: Optional token holdings
            known_successful_wallets: List of known successful wallet addresses

        Returns:
            Dictionary of network features
        """
        features = {}

        try:
            # Centrality features
            if transaction_graph and NETWORKX_AVAILABLE:
                centrality = self._calculate_centrality(wallet_address, transaction_graph)
                features.update(centrality)

            # Token co-holding features
            if token_holdings and known_successful_wallets:
                coholding = self._calculate_coholding_patterns(
                    wallet_address,
                    token_holdings,
                    known_successful_wallets
                )
                features.update(coholding)

            # Clustering features
            if transaction_graph:
                clustering = self._calculate_clustering_features(wallet_address, transaction_graph)
                features.update(clustering)

            features['network_extraction_success'] = True

        except Exception as e:
            logger.error(f"Network feature extraction failed: {e}")
            features['network_extraction_success'] = False

        return features

    def _calculate_centrality(
        self,
        wallet_address: str,
        transaction_graph: Dict[str, Any]
    ) -> Dict[str, float]:
        """Calculate centrality metrics for a wallet."""
        if not NETWORKX_AVAILABLE:
            return {}

        try:
            # Build graph from transaction data
            G = self._build_graph(transaction_graph)

            if wallet_address not in G.nodes:
                return {'centrality_available': False}

            features = {'centrality_available': True}

            # PageRank centrality
            pagerank = nx.pagerank(G, max_iter=100)
            features['pagerank_centrality'] = float(pagerank.get(wallet_address, 0.0))

            # Degree centrality
            degree = nx.degree_centrality(G, normalized=True)
            features['degree_centrality'] = float(degree.get(wallet_address, 0.0))

            # Betweenness centrality (may be slow for large graphs)
            if G.number_of_nodes() < 100:
                betweenness = nx.betweenness_centrality(G, normalized=True)
                features['betweenness_centrality'] = float(betweenness.get(wallet_address, 0.0))

            # Clustering coefficient
            clustering = nx.clustering(G)
            features['local_clustering'] = float(clustering.get(wallet_address, 0.0))

            # Eccentricity (may be slow)
            if G.number_of_nodes() < 100:
                if nx.is_connected(G):
                    eccentricity = nx.eccentricity(G, wallet_address)
                    features['eccentricity'] = float(eccentricity)

            return features

        except Exception as e:
            logger.warning(f"Centrality calculation failed: {e}")
            return {'centrality_available': False, 'error': str(e)}

    def _build_graph(self, transaction_graph: Dict[str, Any]) -> nx.Graph:
        """Build NetworkX graph from transaction data."""
        G = nx.Graph()

        # Add nodes and edges
        for edge in transaction_graph.get('edges', []):
            from_addr = edge.get('from')
            to_addr = edge.get('to')
            weight = edge.get('weight', 1.0)

            if from_addr and to_addr:
                G.add_edge(from_addr, to_addr, weight=weight)

        return G

    def _calculate_coholding_patterns(
        self,
        wallet_address: str,
        token_holdings: Dict[str, float],
        known_successful_wallets: List[str]
    ) -> Dict[str, float]:
        """
        Calculate token co-holding patterns with successful wallets.

        Args:
            wallet_address: Wallet to analyze
            token_holdings: This wallet's token holdings
            known_successful_wallets: List of successful wallet addresses

        Returns:
            Dictionary of co-holding features
        """
        features = {}

        if not token_holdings or not known_successful_wallets:
            return features

        # Calculate overlap with each successful wallet
        overlaps = []
        for success_wallet in known_successful_wallets:
            # In production, you'd fetch their holdings
            # For now, calculate based on shared tokens
            shared_tokens = set(token_holdings.keys())

            # Simulated overlap (in production, use actual data)
            overlap_score = len(shared_tokens) / max(1, len(token_holdings))
            overlaps.append(overlap_score)

        if overlaps:
            features['avg_coholding_with_successful'] = float(np.mean(overlaps))
            features['max_coholding_with_successful'] = float(np.max(overlaps))
            features['successful_wallets_analyzed'] = len(overlaps)

        return features

    def _calculate_clustering_features(
        self,
        wallet_address: str,
        transaction_graph: Dict[str, Any]
    ) -> Dict[str, float]:
        """Calculate clustering and community features."""
        if not NETWORKX_AVAILABLE:
            return {}

        try:
            G = self._build_graph(transaction_graph)

            if wallet_address not in G.nodes:
                return {}

            features = {}

            # Community detection (Louvain)
            if G.number_of_nodes() < 500:
                try:
                    communities = nx.algorithms.community.greedy_modularity_communities(G)

                    # Find which community this wallet is in
                    for i, community in enumerate(communities):
                        if wallet_address in community:
                            features['community_id'] = int(i)
                            features['community_size'] = len(community)
                            break
                except Exception as e:
                    logger.debug(f"Community detection failed: {e}")

            return features

        except Exception as e:
            logger.warning(f"Clustering calculation failed: {e}")
            return {}

    def detect_sybil_wallets(
        self,
        wallet_addresses: List[str],
        transaction_graph: Dict[str, Any],
        threshold: float = 0.8
    ) -> Dict[str, List[str]]:
        """
        Detect sybil wallets using graph clustering.

        Args:
            wallet_addresses: List of wallet addresses to analyze
            transaction_graph: Transaction graph data
            threshold: Similarity threshold for sybil detection

        Returns:
            Dictionary mapping cluster_id to list of sybil wallets
        """
        if not NETWORKX_AVAILABLE:
            logger.warning("NetworkX required for sybil detection")
            return {}

        try:
            G = self._build_graph(transaction_graph)

            # Use connected components or community detection
            communities = nx.connected_components(G)

            sybil_clusters = {}

            for i, community in enumerate(communities):
                # Filter to wallets in our list
                community_wallets = [w for w in community if w in wallet_addresses]

                if len(community_wallets) > 3:  # Suspicious cluster
                    sybil_clusters[f'cluster_{i}'] = community_wallets

            return sybil_clusters

        except Exception as e:
            logger.error(f"Sybil detection failed: {e}")
            return {}

    def calculate_follower_metrics(
        self,
        wallet_address: str,
        follower_graph: Optional[Dict[str, Any]] = None
    ) -> Dict[str, float]:
        """
        Calculate follower network metrics.

        Args:
            wallet_address: Wallet to analyze
            follower_graph: Optional follower/copying graph

        Returns:
            Dictionary of follower metrics
        """
        features = {}

        if not follower_graph:
            return features

        try:
            # Count direct followers
            followers = follower_graph.get('followers', [])
            features['follower_count'] = len(followers)

            # Count following (wallets this wallet copies)
            following = follower_graph.get('following', [])
            features['following_count'] = len(following)

            # Follower quality (how many are successful)
            successful_followers = follower_graph.get('successful_followers', [])
            features['successful_follower_ratio'] = (
                len(successful_followers) / len(followers) if followers else 0.0
            )

            # Copy network centrality
            if NETWORKX_AVAILABLE and follower_graph.get('copy_edges'):
                G = nx.DiGraph()
                for edge in follower_graph['copy_edges']:
                    G.add_edge(edge['from'], edge['to'])

                if wallet_address in G.nodes:
                    # In-degree (number of copiers)
                    in_degree = G.in_degree(wallet_address)
                    features['copier_count'] = in_degree

                    # Out-degree (number being copied)
                    out_degree = G.out_degree(wallet_address)
                    features['copying_count'] = out_degree

        except Exception as e:
            logger.warning(f"Follower metrics calculation failed: {e}")

        return features

    def analyze_token_network(
        self,
        wallet_address: str,
        token_transactions: List[Dict[str, Any]]
    ) -> Dict[str, float]:
        """
        Analyze wallet's position in token transaction network.

        Args:
            wallet_address: Wallet to analyze
            token_transactions: List of token transactions

        Returns:
            Dictionary of token network features
        """
        features = {}

        if not token_transactions:
            return features

        try:
            # Build bipartite graph (wallets <-> tokens)
            if NETWORKX_AVAILABLE:
                G = nx.Graph()

                for tx in token_transactions:
                    wallet = tx.get('wallet')
                    token = tx.get('token')
                    if wallet and token:
                        G.add_edge(wallet, token)

                if wallet_address in G.nodes:
                    # Degree (number of unique tokens traded)
                    degree = G.degree(wallet_address)
                    features['unique_tokens_traded'] = degree

                    # Token centrality
                    if G.number_of_nodes() > 0:
                        centrality = nx.degree_centrality(G)
                        features['token_network_centrality'] = float(centrality.get(wallet_address, 0.0))

            # Token diversity (entropy of token distribution)
            token_counts = Counter(tx.get('token') for tx in token_transactions if tx.get('token'))
            if token_counts:
                total = sum(token_counts.values())
                entropy = -sum(
                    (count / total) * np.log(count / total)
                    for count in token_counts.values()
                )
                max_entropy = np.log(len(token_counts))
                features['token_diversity'] = float(entropy / max_entropy if max_entropy > 0 else 0)

        except Exception as e:
            logger.warning(f"Token network analysis failed: {e}")

        return features

    def extract_network_features_batch(
        self,
        wallet_addresses: List[str],
        wallet_trades_map: Dict[str, List[Dict[str, Any]]]
    ) -> Dict[str, Dict[str, Any]]:
        """
        Extract network features for multiple wallets and analyze network-wide relationships.

        Args:
            wallet_addresses: List of wallet addresses to analyze
            wallet_trades_map: Dictionary mapping wallet addresses to their trade history

        Returns:
            Dictionary mapping wallet addresses to their network features
        """
        network_features = {}
        if not wallet_addresses or not NETWORKX_AVAILABLE:
            return network_features

        try:
            # Build transaction graph from all wallet trades
            G = self._build_multi_wallet_graph(wallet_addresses, wallet_trades_map)

            # Identify successful wallets (high WQS or ACTIVE status)
            successful_wallets = self._identify_successful_wallets(
                wallet_addresses, wallet_trades_map
            )

            # Extract features for each wallet
            for wallet_address in wallet_addresses:
                features = {}

                # Centrality features
                if wallet_address in G.nodes:
                    try:
                        # PageRank centrality
                        pagerank = nx.pagerank(G, max_iter=100)
                        features['pagerank_centrality'] = float(pagerank.get(wallet_address, 0.0))

                        # Degree centrality
                        degree = nx.degree_centrality(G)
                        features['degree_centrality'] = float(degree.get(wallet_address, 0.0))

                        # Betweenness centrality (for smaller graphs)
                        if G.number_of_nodes() < 100:
                            betweenness = nx.betweenness_centrality(G)
                            features['betweenness_centrality'] = float(betweenness.get(wallet_address, 0.0))

                        # Clustering coefficient
                        clustering = nx.clustering(G)
                        features['local_clustering'] = float(clustering.get(wallet_address, 0.0))

                    except Exception as e:
                        logger.debug(f"Centrality calculation failed for {wallet_address[:8]}: {e}")

                # Token co-holding with successful wallets
                if wallet_address in successful_wallets or successful_wallets:
                    trades = wallet_trades_map.get(wallet_address, [])
                    token_holdings = self._extract_token_holdings(trades)
                    coholding_score = self._calculate_coholding_with_successful(
                        wallet_address, token_holdings, successful_wallets, wallet_trades_map
                    )
                    features.update(coholding_score)

                # Community detection
                if G.number_of_nodes() < 500 and wallet_address in G.nodes:
                    try:
                        communities = nx.algorithms.community.greedy_modularity_communities(G)
                        for i, community in enumerate(communities):
                            if wallet_address in community:
                                features['community_id'] = int(i)
                                features['community_size'] = len(community)
                                break
                    except Exception as e:
                        logger.debug(f"Community detection failed: {e}")

                network_features[wallet_address] = features

            # Network-wide sybil detection
            sybil_clusters = self.detect_sybil_wallets(
                wallet_addresses,
                {'edges': list(G.edges(data=True))}
            )
            if sybil_clusters:
                # Add sybil cluster information to affected wallets
                for cluster_id, cluster_wallets in sybil_clusters.items():
                    for wallet in cluster_wallets:
                        if wallet in network_features:
                            network_features[wallet]['sybil_cluster'] = cluster_id
                            network_features[wallet]['sybil_risk'] = 'HIGH'
                        else:
                            network_features[wallet] = {
                                'sybil_cluster': cluster_id,
                                'sybil_risk': 'HIGH'
                            }

            logger.info(f"Computed network features for {len(network_features)} wallets")

        except Exception as e:
            logger.error(f"Batch network feature extraction failed: {e}")

        return network_features

    def _build_multi_wallet_graph(
        self,
        wallet_addresses: List[str],
        wallet_trades_map: Dict[str, List[Dict[str, Any]]]
    ) -> nx.Graph:
        """Build a graph from multiple wallet trades."""
        G = nx.Graph()

        # Add wallet nodes
        for wallet in wallet_addresses:
            G.add_node(wallet)

        # Add edges based on shared tokens and transaction patterns
        for i, wallet1 in enumerate(wallet_addresses):
            for wallet2 in wallet_addresses[i+1:]:
                # Calculate similarity between wallets
                trades1 = wallet_trades_map.get(wallet1, [])
                trades2 = wallet_trades_map.get(wallet2, [])

                if not trades1 or not trades2:
                    continue

                # Extract tokens traded by each wallet
                tokens1 = {tx.get('token') for tx in trades1 if tx.get('token')}
                tokens2 = {tx.get('token') for tx in trades2 if tx.get('token')}

                # Jaccard similarity of token sets
                if tokens1 and tokens2:
                    intersection = len(tokens1 & tokens2)
                    union = len(tokens1 | tokens2)
                    similarity = intersection / union if union > 0 else 0

                    # Add edge if similarity is above threshold
                    if similarity > 0.1:  # At least 10% token overlap
                        G.add_edge(wallet1, wallet2, weight=similarity)

        return G

    def _identify_successful_wallets(
        self,
        wallet_addresses: List[str],
        wallet_trades_map: Dict[str, List[Dict[str, Any]]]
    ) -> List[str]:
        """Identify successful wallets based on trade performance."""
        successful = []

        for wallet in wallet_addresses:
            trades = wallet_trades_map.get(wallet, [])
            if not trades:
                continue

            # Simple success metric: positive PnL trades
            profitable_trades = [tx for tx in trades if tx.get('pnl', 0) > 0]
            success_rate = len(profitable_trades) / len(trades) if trades else 0

            if success_rate > 0.6 and len(trades) >= 5:
                successful.append(wallet)

        return successful

    def _extract_token_holdings(self, trades: List[Dict[str, Any]]) -> Dict[str, float]:
        """Extract token holdings from trades."""
        token_amounts = {}
        for trade in trades:
            token = trade.get('token')
            amount = trade.get('amount', 0)
            if token and amount > 0:
                token_amounts[token] = token_amounts.get(token, 0) + amount
        return token_amounts

    def _calculate_coholding_with_successful(
        self,
        wallet_address: str,
        token_holdings: Dict[str, float],
        successful_wallets: List[str],
        wallet_trades_map: Dict[str, List[Dict[str, Any]]]
    ) -> Dict[str, float]:
        """Calculate token co-holding metrics with successful wallets."""
        features = {}

        if not token_holdings or not successful_wallets:
            return features

        overlaps = []
        for success_wallet in successful_wallets:
            if success_wallet == wallet_address:
                continue

            success_trades = wallet_trades_map.get(success_wallet, [])
            success_holdings = self._extract_token_holdings(success_trades)

            if success_holdings:
                shared_tokens = set(token_holdings.keys()) & set(success_holdings.keys())
                if token_holdings:
                    overlap_score = len(shared_tokens) / len(token_holdings)
                    overlaps.append(overlap_score)

        if overlaps:
            features['avg_coholding_with_successful'] = float(np.mean(overlaps))
            features['max_coholding_with_successful'] = float(np.max(overlaps))
            features['successful_wallets_analyzed'] = len(overlaps)

        return features


# Convenience function
def extract_network_features(
    wallet_address: str,
    transaction_graph: Optional[Dict[str, Any]] = None,
    token_holdings: Optional[Dict[str, float]] = None,
    known_successful_wallets: Optional[List[str]] = None
) -> Dict[str, Any]:
    """
    Quick extraction of network features.

    Args:
        wallet_address: Wallet address
        transaction_graph: Optional transaction graph
        token_holdings: Optional token holdings
        known_successful_wallets: Optional list of successful wallets

    Returns:
        Dictionary of network features
    """
    extractor = NetworkFeatures()
    return extractor.extract_features(
        wallet_address,
        transaction_graph,
        token_holdings,
        known_successful_wallets
    )


def detect_sybil_wallets(
    wallet_addresses: List[str],
    transaction_graph: Dict[str, Any],
    threshold: float = 0.8
) -> Dict[str, List[str]]:
    """
    Convenience function to detect sybil wallets.

    Args:
        wallet_addresses: List of addresses to analyze
        transaction_graph: Transaction graph
        threshold: Similarity threshold

    Returns:
        Dictionary of clusters to sybil wallets
    """
    extractor = NetworkFeatures()
    return extractor.detect_sybil_wallets(wallet_addresses, transaction_graph, threshold)
