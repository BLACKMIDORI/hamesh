package main

import (
	"fmt"
	"github.com/aws/aws-sdk-go/aws"
	"github.com/aws/aws-sdk-go/aws/session"
	"github.com/aws/aws-sdk-go/service/dynamodb"
	"log"
	"strconv"
	"time"
)

var tableName = "hamash-stun-records"

type Record struct {
	subscriptionId string
	creationUnix   int64
	sourceAddress  string
	sourcePort     int64
}

func getDbClient() *dynamodb.DynamoDB {
	// Initialize a session that the SDK will use to load
	// credentials from the shared credentials file ~/.aws/credentials
	// and region from the shared configuration file ~/.aws/config.
	sess := session.Must(session.NewSessionWithOptions(session.Options{
		SharedConfigState: session.SharedConfigEnable,
	}))

	// Create DynamoDB client
	return dynamodb.New(sess)
}

func cleanup(client *dynamodb.DynamoDB) {
	// Remove records older than 1 minute
	result, err := client.Scan(&dynamodb.ScanInput{
		TableName: aws.String(tableName),
		ScanFilter: map[string]*dynamodb.Condition{
			"creation_unix": {
				ComparisonOperator: aws.String(dynamodb.ComparisonOperatorLe),
				AttributeValueList: []*dynamodb.AttributeValue{
					{N: aws.String(fmt.Sprint(time.Now().Unix() - 60))},
				},
			},
		},
	})
	if err != nil {
		log.Fatalf("Got error calling cleanup: %s\n", err)
	}
	for _, item := range result.Items {
		subscriptionId := *item["subscription_id"].S
		creationUnix := *item["creation_unix"].N
		_, err := client.DeleteItem(&dynamodb.DeleteItemInput{
			TableName: aws.String(tableName),
			Key: map[string]*dynamodb.AttributeValue{
				"subscription_id": {S: aws.String(subscriptionId)},
				"creation_unix":   {N: aws.String(creationUnix)},
			},
		})
		if err != nil {
			log.Fatalf("Got error calling cleanup: %s\n", err)
		}
	}
	log.Printf("Cleaned %v records\n", *result.Count)
}

func storeRecord(client *dynamodb.DynamoDB, record Record) {
	// Remove records older than 1 minute
	result, err := client.PutItem(&dynamodb.PutItemInput{
		TableName: aws.String(tableName),
		Item: map[string]*dynamodb.AttributeValue{
			"subscription_id": {S: aws.String(record.subscriptionId)},
			"creation_unix":   {N: aws.String(fmt.Sprint(record.creationUnix))},
			"source_address":  {S: aws.String(record.sourceAddress)},
			"source_port":     {N: aws.String(fmt.Sprint(record.sourcePort))},
		},
	})
	if err != nil {
		log.Fatalf("Got error calling storeRecord: %s\n", err)
	}
	log.Printf("Recorded '$%v': %v", result.String(), record)
}

func findRecordsBySubscriptionId(client *dynamodb.DynamoDB, subscriptionId string) []Record {
	result, err := client.Query(&dynamodb.QueryInput{
		TableName: aws.String(tableName),
		KeyConditions: map[string]*dynamodb.Condition{
			"subscription_id": {
				ComparisonOperator: aws.String(dynamodb.ComparisonOperatorEq),
				AttributeValueList: []*dynamodb.AttributeValue{
					{S: aws.String(subscriptionId)},
				},
			},
		},
	})
	if err != nil {
		log.Fatalf("Got error calling findRecordsBySubscriptionId: %s\n", err)
	}
	log.Printf("Received %v records\n", *result.Count)

	var records []Record
	for _, item := range result.Items {
		creationUnix, _ := strconv.ParseInt(*item["creation_unix"].N, 10, 64)
		sourcePort, _ := strconv.ParseInt(*item["source_port"].N, 10, 64)
		record := Record{
			subscriptionId: *item["subscription_id"].S,
			creationUnix:   creationUnix,
			sourceAddress:  *item["source_address"].S,
			sourcePort:     sourcePort,
		}
		records = append(records, record)
	}
	return records
}
