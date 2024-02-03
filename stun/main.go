package main

import (
	"fmt"
	"github.com/a-h/awsapigatewayv2handler"
	"github.com/aws/aws-lambda-go/lambda"
	"net/http"
	"os"
)

var httpLambdaHandler lambda.Handler

func init() {
	http.HandleFunc("/subscription", subscribeHandler)
	http.HandleFunc("/subscription/", checkSubscriptionHandler)
	http.HandleFunc("/join/", joinHandler)

	httpLambdaHandler = awsapigatewayv2handler.NewLambdaHandler(http.DefaultServeMux)
}

func main() {
	if isRunningOnLambda() {
		// Run on AWS Lambda
		lambda.Start(httpLambdaHandler)
	} else {
		// Run local server
		fmt.Println("Running on 0.0.0.0:8080")
		_ = http.ListenAndServe("0.0.0.0:8080", http.DefaultServeMux)
	}
}

func isRunningOnLambda() bool {
	return os.Getenv("AWS_LAMBDA_RUNTIME_API") != ""
}
