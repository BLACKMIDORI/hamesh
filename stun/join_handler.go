package main

import (
	"encoding/json"
	"log"
	"net/http"
	"strconv"
	"strings"
	"time"
)

func joinHandler(w http.ResponseWriter, r *http.Request) {
	log.Printf("Handling: %v\n", r.URL)
	version := r.URL.Query().Get("version")
	if version == "" {
		w.WriteHeader(400)
		_ = json.NewEncoder(w).Encode(map[string]any{
			"error": map[string]any{"message": "query param 'version' cannot be empty."},
		})
		return
	}
	if version != "1" {
		w.WriteHeader(400)
		_ = json.NewEncoder(w).Encode(map[string]any{
			"error": map[string]any{"message": "unexpected version"},
		})
		return
	}
	sourceAddressAndPort := r.Header.Get("Cloudfront-Viewer-Address")
	log.Printf("Source: %v\n", sourceAddressAndPort)
	params := routeParams(r, "/join/{subscriptionId}")
	subscriptionId := params["subscriptionId"]
	dbClient := getDbClient()
	cleanup(dbClient)
	addressAndPortParts := strings.Split(sourceAddressAndPort, ":")
	sourcePortStr := addressAndPortParts[len(addressAndPortParts)-1]
	sourcePort, _ := strconv.ParseInt(sourcePortStr, 10, 64)
	sourceAddress := sourceAddressAndPort[:len(sourceAddressAndPort)-len(sourcePortStr)-1]
	now := time.Now()
	records := findRecordsBySubscriptionId(dbClient, subscriptionId)
	if len(records) == 0 {
		w.WriteHeader(400)
		_ = json.NewEncoder(w).Encode(map[string]any{
			"error": map[string]any{
				"message": "Subscription not found. Could not join.",
			},
		})
		return
	} else if len(records) > 1 {
		notFound := true
		for _, record := range records {
			if record.sourceAddress == sourceAddress && record.sourcePort == sourcePort {
				notFound = false
			}
		}
		if notFound {
			w.WriteHeader(403)
			_ = json.NewEncoder(w).Encode(map[string]any{
				"error": map[string]any{
					"message": "This subscription already has a pair. Could not join.",
				},
			})
			return
		}
	} else if records[0].sourceAddress != sourceAddress || records[0].sourcePort != sourcePort {
		record := Record{
			subscriptionId: subscriptionId,
			creationUnix:   now.Unix(),
			sourceAddress:  sourceAddress,
			sourcePort:     sourcePort,
		}
		storeRecord(dbClient, record)
	}
	_ = json.NewEncoder(w).Encode(map[string]any{
		"value": map[string]any{
			"subscriptionId": subscriptionId,
		},
	})
}
