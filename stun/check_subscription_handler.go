package main

import (
	"encoding/json"
	"log"
	"net/http"
)

type RecordDto struct {
	Address string `json:"address"`
	Port    int64  `json:"port"`
}

func checkSubscriptionHandler(w http.ResponseWriter, r *http.Request) {
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
	params := routeParams(r, "/subscription/{subscriptionId}")
	subscriptionId := params["subscriptionId"]
	log.Printf("subscriptionId: %v\n", subscriptionId)
	dbClient := getDbClient()
	cleanup(dbClient)
	records := findRecordsBySubscriptionId(dbClient, subscriptionId)
	list := make([]RecordDto, 0)
	for _, record := range records {
		list = append(list, RecordDto{record.sourceAddress, record.sourcePort})
	}
	_ = json.NewEncoder(w).Encode(map[string]any{
		"value": map[string]any{
			"peers": list,
		},
	})
}
