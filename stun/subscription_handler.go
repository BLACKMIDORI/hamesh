package main

import (
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"hash/crc32"
	"log"
	"net/http"
	"strconv"
	"strings"
	"time"
)

func subscribeHandler(w http.ResponseWriter, r *http.Request) {
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
	dbClient := getDbClient()
	cleanup(dbClient)
	addressAndPortParts := strings.Split(sourceAddressAndPort, ":")
	sourcePortStr := addressAndPortParts[len(addressAndPortParts)-1]
	sourcePort, _ := strconv.ParseInt(sourcePortStr, 10, 64)
	sourceAddress := sourceAddressAndPort[:len(sourceAddressAndPort)-len(sourcePortStr)-1]
	now := time.Now()
	digest := sha256.Sum256([]byte(fmt.Sprintln(sourceAddress, now.Unix())))
	checksum := crc32.ChecksumIEEE(digest[:])
	subscriptionId := fmt.Sprint(checksum)
	records := findRecordsBySubscriptionId(dbClient, subscriptionId)
	if len(records) > 0 {
		log.Printf("Conflict found, subscriptionId: %v\n", subscriptionId)
		log.Printf("%v records", len(records))
		w.WriteHeader(409)
		_ = json.NewEncoder(w).Encode(map[string]any{
			"error": map[string]any{
				"message": "Conflict. Try again.",
			},
		})
		return
	}
	record := Record{
		subscriptionId: subscriptionId,
		creationUnix:   now.Unix(),
		sourceAddress:  sourceAddress,
		sourcePort:     sourcePort,
	}
	storeRecord(dbClient, record)
	_ = json.NewEncoder(w).Encode(map[string]any{
		"value": map[string]any{
			"subscriptionId": subscriptionId,
			"sourceAddress":  sourceAddress,
			"sourcePort":     sourcePort,
		},
	})
}
